//! Per-particle position modulation sources.
//!
//! All sources produce a *t-space offset* around the particle's fixed onset.
//! The engine smooths the resulting target so abrupt mod changes never click.

use std::sync::OnceLock;

use crate::rng::SplitMix64;

/// Cheap sine approximation via a 4096-entry lookup table with linear
/// interpolation. The engine calls this per particle per sample, so the
/// libm sin() cost at 256+ particles / 48 kHz is measurable.
fn sin_approx(phase: f32) -> f32 {
    static TABLE: OnceLock<[f32; 4096]> = OnceLock::new();
    let t = TABLE.get_or_init(|| {
        core::array::from_fn(|i| {
            let a = (i as f32 / 4096.0) * std::f32::consts::TAU;
            a.sin()
        })
    });
    let x = phase * (4096.0 / std::f32::consts::TAU);
    let i0 = (x.floor() as i64).rem_euclid(4096) as usize;
    let i1 = (i0 + 1) & 4095;
    let frac = x - x.floor();
    t[i0] + (t[i1] - t[i0]) * frac
}

/// Position modulation source owned by one particle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionMod {
    /// Keep reading at the spawn onset.
    Fixed,
    /// Slow sine wobble around the onset.
    Lfo { depth: f32, rate_hz: f32, phase: f32 },
    /// Bounded random walk around the onset, stepped at fixed intervals.
    RandomWalk {
        depth: f32,
        step: f32,
        interval_samples: usize,
        next_walk: usize,
        value: f32,
    },
    /// Follow the engine's shared peak position (t of the loudest sample in
    /// the recent history window; updated periodically, see
    /// ParticulaEngine::peak_* parameters). The engine passes that target in
    /// each sample, so this variant carries no per-particle state.
    PeakFollow,
}

impl PositionMod {
    /// Fixed position modulation.
    pub fn fixed() -> Self {
        Self::Fixed
    }

    /// Sine LFO with a random initial phase.
    pub fn lfo(rate_hz: f32, depth: f32, rng: &mut SplitMix64) -> Self {
        Self::Lfo {
            depth: depth.clamp(0.0, 1.0),
            rate_hz: rate_hz.abs(),
            phase: rng.range(0.0, std::f32::consts::TAU),
        }
    }

    /// Follow the engine-level recent peak position.
    pub fn peak_follow() -> Self {
        Self::PeakFollow
    }

    /// Random walk around the onset, re-stepped every `interval_samples`.
    pub fn random_walk(
        step: f32,
        depth: f32,
        interval_samples: usize,
        rng: &mut SplitMix64,
    ) -> Self {
        Self::RandomWalk {
            depth: depth.clamp(0.0, 1.0),
            step: step.abs(),
            interval_samples: interval_samples.max(1),
            next_walk: 0,
            value: rng.sym() * 0.01,
        }
    }

    /// Next modulation *offset* in t-space. The caller adds it to the onset,
    /// clamps to `[0, 1]` and feeds it into the particle's smoother.
    pub fn next_offset(&mut self, dt: f32, sample_count: usize, rng: &mut SplitMix64) -> f32 {
        match self {
            Self::Fixed | Self::PeakFollow => 0.0,
            Self::Lfo { depth, rate_hz, phase } => {
                *phase += *rate_hz * std::f32::consts::TAU * dt;
                *depth * sin_approx(*phase)
            }
            Self::RandomWalk {
                depth,
                step,
                interval_samples,
                next_walk,
                value,
            } => {
                if sample_count >= *next_walk {
                    *next_walk = sample_count + *interval_samples;
                    *value += rng.sym() * *step;
                    *value = value.clamp(-*depth, *depth);
                }
                *value
            }
        }
    }
}
