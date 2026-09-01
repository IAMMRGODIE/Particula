//! A tiny, seedable, deterministic RNG for the audio thread.
//!
//! SplitMix64: trivially correct, no dependencies, deterministic across runs
//! for a fixed seed (needed for reproducible experimental patches).

/// SplitMix64 pseudo random generator.
#[derive(Clone, Copy, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// Creates a new generator from a seed.
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Returns the next 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` with 24 bits of mantissa precision.
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0 / (1u64 << 24) as f32)
    }

    /// Uniform in `[min, max)`.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }

    /// Uniform in `[min, max]` (inclusive), clamped so `min <= max` holds.
    pub fn range_usize(&mut self, min: usize, max: usize) -> usize {
        let (lo, hi) = (min.min(max), min.max(max));
        lo + (self.next_u64() % (hi - lo + 1) as u64) as usize
    }

    /// Uniform in `[-1, 1)`.
    pub fn sym(&mut self) -> f32 {
        self.range(-1.0, 1.0)
    }
}
