//! The particle cloud engine: shared history + particle pool + spawn rule.

use std::f32::consts::PI;

use i_am_dsp::{Effect, ProcessContext, tools::ring_buffer::RingBuffer};
use i_am_dsp_derive::Parameters;

use crate::{
    history::recent_peak_position,
    particle::Particle,
    position_mod::PositionMod,
    rng::SplitMix64,
    spawner::Spawner,
    texture::Texture,
};

/// Fixed pool capacity of the slot map (Architecture.md sec.10: 64~256).
pub const DEFAULT_POOL_CAPACITY: usize = 256;

/// The core engine. Mono for now (one shared history, particles sum into the
/// single output channel).
///
/// Data flow per sample (see Architecture.md sec.2):
/// 1. dry input pushed at the write head;
/// 2. spawner rules decide whether a particle is born (arithmetic position
///    sequence + jitter + exponential strength decay);
/// 3. every live particle reads the history (optionally blended with the
///    WSOLA texture layer, v2) at its smoothed position with cubic
///    interpolation, applies playback rate, IIR-Hilbert frequency shift, the
///    envelope, and writes feedback back into the history (v1, serial
///    semantics);
/// 4. output = dry * input + wet.
///
/// No BPM sync yet.
#[derive(Parameters)]
pub struct ParticulaEngine {
    // --- live-tweakable parameters (host-visible) ---
    #[range(min = 0.0, max = 1.0)]
    pub dry: f32,
    #[range(min = 0.0, max = 1.0)]
    pub wet: f32,

    #[range(min = 1.0, max = 5000.0)]
    pub spawn_interval_ms: f32,
    #[range(min = 1.0, max = 256.0)]
    pub max_particles: f32,

    // Spawn rule: arithmetic position sequence + jitter + exp strength decay.
    #[range(min = 0.0, max = 1.0)]
    pub base_position: f32,
    #[range(min = -1.0, max = 1.0)]
    pub position_step: f32,
    #[range(min = 0.0, max = 1.0)]
    pub position_jitter: f32,
    #[range(min = 0.001, max = 1.0)]
    #[logarithmic]
    pub gain_decay_ratio: f32,
    #[range(min = 0.0, max = 1.0)]
    pub initial_gain: f32,

    // Particle shape.
    #[range(min = 0.0, max = 100.0)]
    pub attack_ms: f32,
    #[range(min = 1.0, max = 10000.0)]
    pub lifetime_ms_min: f32,
    #[range(min = 1.0, max = 10000.0)]
    pub lifetime_ms_max: f32,
    #[range(min = 0.05, max = 8.0)]
    #[logarithmic]
    pub pitch_min: f32,
    #[range(min = 0.05, max = 8.0)]
    #[logarithmic]
    pub pitch_max: f32,
    #[range(min = -5000.0, max = 5000.0)]
    pub freq_shift_min: f32,
    #[range(min = -5000.0, max = 5000.0)]
    pub freq_shift_max: f32,

    // Position smoothing.
    #[range(min = 0.1, max = 1000.0)]
    pub position_smooth_ms: f32,

    // Position modulation source (0 = fixed, 1 = LFO, 2 = random walk,
    // 3 = peak follow).
    #[range(min = 0, max = 3)]
    pub position_mode: i32,
    #[range(min = 0.01, max = 50.0)]
    pub lfo_rate_hz: f32,
    #[range(min = 0.0, max = 0.5)]
    pub lfo_depth: f32,
    #[range(min = 0.0, max = 0.25)]
    pub random_walk_step: f32,
    #[range(min = 1.0, max = 2000.0)]
    pub random_walk_interval_ms: f32,
    #[range(min = 1.0, max = 2000.0)]
    pub peak_window_ms: f32,
    #[range(min = 1.0, max = 1000.0)]
    pub peak_update_ms: f32,
    #[range(min = 0.0, max = 1.0)]
    pub peak_threshold: f32,

    // Feedback (v1).
    #[range(min = 0.0, max = 0.99)]
    pub feedback_gain: f32,
    #[range(min = 0.0, max = 2000.0)]
    pub feedback_delay_ms: f32,
    #[range(min = 0.0, max = 20000.0)]
    pub feedback_damping_hz: f32,

    // WSOLA texture layer (v2).
    #[range(min = 0.0, max = 1.0)]
    pub texture_blend: f32,
    #[range(min = 20.0, max = 2000.0)]
    pub texture_window_ms: f32,
    #[range(min = 5.0, max = 1000.0)]
    pub texture_refresh_ms: f32,
    #[range(min = 0.25, max = 4.0)]
    #[logarithmic]
    pub texture_stretch: f32,
    #[range(min = 1.0, max = 200.0)]
    pub texture_crossfade_ms: f32,

    // --- internal state (never host parameters) ---
    #[skip]
    history: RingBuffer<f32>,
    #[skip]
    texture: Texture,
    #[skip]
    slots: Vec<Option<Particle>>,
    #[skip]
    free: Vec<usize>,
    #[skip]
    spawner: Spawner,
    #[skip]
    rng: SplitMix64,
    #[skip]
    sample_count: usize,
    #[skip]
    sample_rate: usize,
    #[skip]
    spawn_count: usize,
    #[skip]
    peak_position_t: f32,
    #[skip]
    next_peak_update: usize,
}

impl Default for ParticulaEngine {
    fn default() -> Self {
        Self::new(1 << 15, 48_000, 0x5EED_FA11)
    }
}

impl ParticulaEngine {
    /// Creates a new engine.
    ///
    /// history_capacity bounds the maximum readable delay in samples;
    /// changing it during a run invalidates the buffer content (see
    /// Architecture.md sec.3.2), so pick it up front.
    pub fn new(history_capacity: usize, sample_rate: usize, seed: u64) -> Self {
        Self {
            dry: 1.0,
            wet: 0.8,
            spawn_interval_ms: 40.0,
            max_particles: 64.0,
            base_position: 0.5,
            position_step: 0.0,
            position_jitter: 0.02,
            gain_decay_ratio: 0.9,
            initial_gain: 0.5,
            attack_ms: 10.0,
            lifetime_ms_min: 200.0,
            lifetime_ms_max: 1500.0,
            pitch_min: 0.5,
            pitch_max: 1.5,
            freq_shift_min: -120.0,
            freq_shift_max: 120.0,
            position_smooth_ms: 20.0,
            position_mode: 1,
            lfo_rate_hz: 0.15,
            lfo_depth: 0.15,
            random_walk_step: 0.02,
            random_walk_interval_ms: 200.0,
            peak_window_ms: 150.0,
            peak_update_ms: 30.0,
            peak_threshold: 0.01,
            feedback_gain: 0.0,
            feedback_delay_ms: 40.0,
            feedback_damping_hz: 3000.0,
            texture_blend: 0.35,
            texture_window_ms: 85.0,
            texture_refresh_ms: 43.0,
            texture_stretch: 1.0,
            texture_crossfade_ms: 12.0,
            history: RingBuffer::new(history_capacity.max(1)),
            texture: Texture::new(
                (0.085 * sample_rate as f32) as usize,
                sample_rate.max(1),
            ),
            slots: Vec::with_capacity(DEFAULT_POOL_CAPACITY),
            free: Vec::with_capacity(DEFAULT_POOL_CAPACITY),
            spawner: Spawner::new(),
            rng: SplitMix64::new(seed),
            sample_count: 0,
            sample_rate,
            spawn_count: 0,
            peak_position_t: 0.5,
            next_peak_update: 0,
        }
    }

    /// Reseeds the RNG (reproducible patches).
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = SplitMix64::new(seed);
    }

    /// Clears the pool and the spawn sequence (keeps history).
    pub fn clear_particles(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.spawner.reset(0);
    }

    /// Number of live particles right now.
    pub fn live_count(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    /// Total particles born since construction / last clear.
    pub fn spawned(&self) -> usize {
        self.spawn_count
    }

    /// Samples processed since construction / last clear.
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// The spawn rule's arithmetic position for generation n
    /// (t-space, wrapped). Pure: jitter = 0 gives the exact sequence.
    pub fn spawn_rule_position(&self, n: usize) -> f32 {
        (self.base_position + n as f32 * self.position_step).rem_euclid(1.0)
    }

    /// The spawn rule's strength (linear gain) for generation n:
    /// initial_gain * decay_ratio^n — the exponential decay law.
    pub fn spawn_rule_gain(&self, n: usize) -> f32 {
        self.initial_gain * self.gain_decay_ratio.powi(n as i32)
    }

    /// Builds one particle from the current parameters (spawn rule + shape).
    fn make_particle(&mut self, sample_rate: usize) -> Particle {
        let n = self.spawner.sequence_index();
        let position = (self.spawn_rule_position(n)
            + self.position_jitter * self.rng.sym())
        .rem_euclid(1.0);
        let gain = self.spawn_rule_gain(n);
        let rate = self.rng.range(self.pitch_min, self.pitch_max);
        let shift = self.rng.range(self.freq_shift_min, self.freq_shift_max);
        let lmin = (self.lifetime_ms_min * sample_rate as f32 / 1000.0) as usize;
        let lmax = (self.lifetime_ms_max * sample_rate as f32 / 1000.0) as usize;
        let lifetime = self.rng.range_usize(lmin, lmax);
        let attack = ((self.attack_ms * sample_rate as f32 / 1000.0).max(1.0)) as usize;
        let mode = match self.position_mode {
            0 => PositionMod::fixed(),
            1 => PositionMod::lfo(self.lfo_rate_hz, self.lfo_depth, &mut self.rng),
            2 => PositionMod::random_walk(
                self.random_walk_step,
                self.lfo_depth,
                (self.random_walk_interval_ms * sample_rate as f32 / 1000.0) as usize,
                &mut self.rng,
            ),
            _ => PositionMod::peak_follow(),
        };
        Particle::new(
            sample_rate,
            position,
            mode,
            rate,
            shift,
            gain,
            self.feedback_gain,
            attack,
            lifetime,
            self.position_smooth_ms,
        )
    }

    fn set_sample_rate(&mut self, sample_rate: usize) {
        // v0 limitation: existing particles keep their construction sample
        // rate (their Hilbert/Biquad coefficients); a sample-rate change only
        // affects newly spawned particles. TODO: rebuild live particle state.
        self.sample_rate = sample_rate;
    }
}

impl Effect<1> for ParticulaEngine {
    fn delay(&self) -> usize {
        // No FIR / WSOLA yet: zero latency.
        0
    }

    fn process(
        &mut self,
        samples: &mut [f32; 1],
        _other: &[&[f32; 1]],
        ctx: &mut Box<dyn ProcessContext>,
    ) {
        let infos = ctx.infos();
        let new_sr = if infos.trustable && infos.sample_rate != 0 {
            infos.sample_rate
        } else {
            48_000
        };
        if new_sr != self.sample_rate {
            self.set_sample_rate(new_sr);
        }
        let sample_rate = self.sample_rate;

        // 1. dry input at the write head.
        let input = samples[0];
        self.history.push(input);
        self.sample_count += 1;

        // 2. spawn scheduling (pure parameter-driven, audio thread resident;
        //    see Architecture.md sec.6).
        let interval = ((self.spawn_interval_ms * sample_rate as f32 / 1000.0).max(1.0)) as usize;
        if self.spawner.poll(self.sample_count, interval) {
            self.spawner.bump_sequence();
            let lives = self.live_count();
            if lives < self.max_particles.max(1.0) as usize && lives < DEFAULT_POOL_CAPACITY {
                let idx = self
                    .free
                    .pop()
                    .unwrap_or_else(|| {
                        self.slots.push(None);
                        self.slots.len() - 1
                    });
                self.slots[idx] = Some(self.make_particle(sample_rate));
                self.spawn_count += 1;
            }
        }

        // 3. shared peak-follow target (periodic update of the loudest
        //    sample in the recent history window).
        let update = ((self.peak_update_ms * sample_rate as f32 / 1000.0) as usize).max(1);
        if self.sample_count >= self.next_peak_update {
            let window = ((self.peak_window_ms * sample_rate as f32 / 1000.0) as usize).max(1);
            self.peak_position_t = recent_peak_position(&self.history, window, self.peak_threshold);
            self.next_peak_update = self.sample_count + update;
        }

        // 3b. WSOLA texture layer: slide the tap, refresh on schedule.
        let window_samples = ((self.texture_window_ms * sample_rate as f32 / 1000.0) as usize).max(64);
        if window_samples != self.texture.window_capacity() {
            self.texture.resize(window_samples);
        }
        let refresh = ((self.texture_refresh_ms * sample_rate as f32 / 1000.0) as usize).max(1);
        let fade = ((self.texture_crossfade_ms * sample_rate as f32 / 1000.0) as usize).max(1);
        self.texture.process(
            &self.history,
            self.texture_stretch,
            refresh,
            fade,
            self.sample_count,
        );

        // 4. particles: read, pitch, shift, envelope, sum; serial feedback
        //    writes into the history (Architecture.md sec.3.1: later
        //    particles see earlier ones' feedback this frame).
        let mut wet = 0.0_f32;
        let dt = 1.0 / sample_rate as f32;
        let feedback_delay = ((self.feedback_delay_ms * sample_rate as f32 / 1000.0) as usize)
            .min(self.history.capacity());
        let fb_lp_a = if self.feedback_damping_hz <= 0.0 {
            1.0
        } else {
            1.0 - (-2.0 * PI * self.feedback_damping_hz / sample_rate as f32).exp()
        };
        let peak_t = self.peak_position_t;
        let sample_count = self.sample_count;
        let (rng, history) = (&mut self.rng, &mut self.history);
        let mut i = 0;
        while i < self.slots.len() {
            let Some(p) = &mut self.slots[i] else {
                i += 1;
                continue;
            };
            match p.process(
                history,
                &self.texture,
                self.texture_blend,
                dt,
                sample_count,
                peak_t,
                feedback_delay,
                fb_lp_a,
                rng,
                ctx,
            ) {
                Some(s) => wet += s,
                None => {
                    self.slots[i] = None;
                    self.free.push(i);
                },
            }
            i += 1;
        }

        // 5. dry + wet.
        samples[0] = input * self.dry + wet;
    }
}