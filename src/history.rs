//! History read/write helpers (the shared delay line).
//!
//! Everything is expressed in the RingBuffer's own orientation: index 0 is the
//! oldest sample, index `capacity-1` the freshest (the dry input just pushed).

use i_am_dsp::tools::ring_buffer::RingBuffer;

/// Adds `value` to history `delay_samples` behind the freshest sample.
///
/// `delay_samples = 0` targets the just-written dry input slot
/// (Architecture.md §3.1, the pseudocode `history[-1]`). Used by the serial
/// feedback path; values are expected to be pre-clipped/soft-clamped by the
/// caller (Architecture.md §8 stability trio).
pub fn add_at(history: &mut RingBuffer<f32>, delay_samples: usize, value: f32) {
    let cap = history.capacity();
    if cap == 0 {
        return;
    }
    let delay_samples = delay_samples.min(cap - 1);
    let newest = (history.current_pos() + cap - 1) % cap; // freshest slot
    let idx = (newest + cap - delay_samples) % cap;
    let buf = history.underlying_buffer_mut();
    buf[idx] += value;
}

/// t-space position (0 = oldest, 1 = freshest) of the loudest sample in the
/// most recent `window_samples` of history.
///
/// Samples with magnitude below `threshold` are ignored; if nothing passes
/// the threshold the freshest position (1.0) is returned as a fallback.
pub fn recent_peak_position(
    history: &RingBuffer<f32>,
    window_samples: usize,
    threshold: f32,
) -> f32 {
    let cap = history.capacity();
    if cap == 0 {
        return 0.0;
    }
    let window = window_samples.min(cap);
    let mut best_t = 1.0; // fallback: freshest
    let mut best = threshold;
    for i in 0..window {
        // Walk from the freshest slot backwards.
        let slot = (history.current_pos() + cap - 1 - i) % cap;
        let v = history[slot].abs();
        if v > best {
            best = v;
            let head_relative = cap - 1 - i;
            best_t = head_relative as f32 / (cap - 1) as f32;
        }
    }
    best_t
}
