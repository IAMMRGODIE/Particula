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

/// Linear-interpolated history read at t-space position (0 = oldest,
/// 1 = freshest). The particle hot loop uses this instead of the library's
/// cubic `WaveTable::sample` — with 128+ voices the extra two reads and the
/// cubic weights per sample per particle are measurable.
pub fn read_linear(history: &RingBuffer<f32>, t: f32) -> f32 {
    let cap = history.capacity();
    if cap == 0 {
        return 0.0;
    }
    // History capacity is a power of two (1 << 16), so a mask does the
    // wrap-around branch-free — a per-sample `rem_euclid` here showed up
    // badly at 300+ voices.
    debug_assert!(cap.is_power_of_two());
    let mask = cap - 1;
    let freshest = (history.current_pos() + mask) & mask;
    let off = t.clamp(0.0, 1.0) * (cap - 1) as f32;
    let slot_f = freshest as f32 - off;
    let f0 = slot_f.floor();
    let i0 = (f0 as isize) & mask as isize;
    let i0 = i0 as usize;
    let i1 = (i0 + 1) & mask;
    let frac = slot_f - f0;
    history[i0] + (history[i1] - history[i0]) * frac
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
