//! WSOLA texture layer (v2, Architecture.md sec.9).
//!
//! A continuously refreshed, time-stretched wavetable that particles can
//! blend in as an alternative read source. The texture keeps its own small
//! sliding window tapped from the freshest history samples; on a fixed
//! refresh schedule it runs the batch WSOLA stretch over that window. The
//! stretched result is a frozen wavetable between refreshes.
//!
//! Every refresh crossfades the old stretched buffer into the new one
//! (over `fade_samples`), which smooths both the sliding source motion and
//! abrupt `stretch` parameter changes - no zipper/click at refresh edges.

use i_am_dsp::{
    generators::wavetable::WaveTable,
    prelude::negative_mean_square_error,
    tools::{ring_buffer::RingBuffer, wsola::wsola},
};

/// The texture layer.
pub struct Texture {
    /// Sliding tap over the freshest history samples.
    window: RingBuffer<f32>,
    /// Latest stretched wavetable (read in t-space via WaveTable).
    stretched: Vec<f32>,
    /// Previous stretched wavetable during a crossfade.
    prev: Option<Vec<f32>>,
    fade_remaining: usize,
    fade_total: usize,
    refresh_interval: usize,
    next_refresh: usize,
    hop: usize,
    ref_range: usize,
    max_offset: usize,
    last_stretch: f32,
    refreshes: usize,
}

impl Texture {
    /// Creates a new texture with the given window capacity (samples).
    pub fn new(window_capacity: usize, sample_rate: usize) -> Self {
        let cap = window_capacity.max(64);
        Self {
            window: RingBuffer::new(cap),
            stretched: Vec::new(),
            prev: None,
            fade_remaining: 0,
            fade_total: ((0.012 * sample_rate as f32) as usize).max(1),
            refresh_interval: (cap / 2).max(1),
            next_refresh: 0,
            hop: (cap / 4).max(1),
            ref_range: ((cap / 8).max(1)).min(cap / 4),
            max_offset: 8,
            last_stretch: 1.0,
            refreshes: 0,
        }
    }

    /// Current window capacity in samples.
    pub fn window_capacity(&self) -> usize {
        self.window.capacity()
    }

    /// Drops the stretched texture entirely (PANIC): reads return 0 until
    /// the next refresh rebuilds it.
    pub fn clear(&mut self) {
        self.stretched.clear();
        self.prev = None;
        self.fade_remaining = 0;
    }

    /// Number of WSOLA refreshes performed so far.
    pub fn refreshes(&self) -> usize {
        self.refreshes
    }

    /// Resizes the sliding window (clears texture content; call when the
    /// window size parameter changes).
    pub fn resize(&mut self, window_capacity: usize) {
        let cap = window_capacity.max(64);
        if cap == self.window.capacity() {
            return;
        }
        self.window = RingBuffer::new(cap);
        self.stretched.clear();
        self.prev = None;
        self.next_refresh = 0;
        self.hop = (cap / 4).max(1);
        self.ref_range = ((cap / 8).max(1)).min(cap / 4);
    }

    /// Advance one sample: slide the tap along the freshest history sample,
    /// refresh the stretched texture on schedule, advance the crossfade.
    pub fn process(
        &mut self,
        history: &RingBuffer<f32>,
        stretch: f32,
        refresh_interval: usize,
        fade_samples: usize,
        sample_count: usize,
    ) {
        let cap = self.window.capacity();
        if cap == 0 {
            return;
        }

        // 1. Track the freshest history sample (ring orientation: capacity-1
        //    is the just-written sample).
        if history.capacity() > 0 {
            self.window.push(history[history.capacity() - 1]);
        } else {
            self.window.push(0.0);
        }

        self.refresh_interval = refresh_interval.max(1);
        self.fade_total = fade_samples.max(1);

        // 2. Scheduled batch WSOLA refresh.
        if sample_count >= self.next_refresh {
            self.next_refresh = sample_count + self.refresh_interval;
            let prev_tail: &[f32] = if self.stretched.len() >= 2 * self.hop {
                let end = (self.stretched.len() / 2 + self.hop).min(self.stretched.len());
                &self.stretched[..end]
            } else {
                &[]
            };
            let new = wsola(
                &self.window,
                prev_tail,
                stretch,
                self.max_offset,
                self.hop,
                self.ref_range,
                negative_mean_square_error,
            );
            if !self.stretched.is_empty() {
                // Always crossfade old -> new so source motion and stretch
                // changes cannot click at refresh edges.
                self.prev = Some(std::mem::replace(&mut self.stretched, new));
                self.fade_remaining = self.fade_total;
            } else {
                self.stretched = new;
            }
            self.last_stretch = stretch;
            self.refreshes += 1;
        }

        // 3. Advance the crossfade.
        if self.fade_remaining > 0 {
            self.fade_remaining -= 1;
            if self.fade_remaining == 0 {
                self.prev = None;
            }
        }
    }

    /// Linear-interpolated read (used by the particle hot loop; avoids the
    /// library's cubic `WaveTable::sample` per voice per sample).
    pub fn sample_linear(&self, t: f32) -> f32 {
        fn lerp_vec(v: &[f32], t: f32) -> f32 {
            let n = v.len();
            if n == 0 {
                return 0.0;
            }
            if n == 1 {
                return v[0];
            }
            let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i0 = x.floor() as usize;
            let i1 = (i0 + 1).min(n - 1);
            let f = x - x.floor();
            v[i0] + (v[i1] - v[i0]) * f
        }
        if self.stretched.is_empty() {
            return 0.0;
        }
        let cur = lerp_vec(&self.stretched, t);
        if let Some(prev) = &self.prev {
            let progress = 1.0 - self.fade_remaining as f32 / self.fade_total as f32;
            let a = lerp_vec(prev, t);
            cur * progress + a * (1.0 - progress)
        } else {
            cur
        }
    }

    /// Read the texture at t in [0, 1) (crossfades during a refresh).
    /// Returns 0 while no texture has been stretched yet.
    pub fn sample(&self, t: f32) -> f32 {
        if self.stretched.is_empty() {
            return 0.0;
        }
        let cur = self.stretched.sample(t, 0);
        if let Some(prev) = &self.prev {
            let progress = 1.0 - self.fade_remaining as f32 / self.fade_total as f32;
            let a = prev.sample(t, 0);
            cur * progress + a * (1.0 - progress)
        } else {
            cur
        }
    }
}