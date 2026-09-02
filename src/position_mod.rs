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

/// LFO waveform shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LfoWave {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl From<i32> for LfoWave {
    fn from(i: i32) -> Self {
        match i.clamp(0, 3) {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            _ => Self::Square,
        }
    }
}

/// Position modulation source owned by one particle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionMod {
    /// Keep reading at the spawn onset.
    Fixed,
    /// Wobble around the onset with a shaped oscillator.
    Lfo {
        depth: f32,
        rate_hz: f32,
        phase: f32,
        wave: LfoWave,
    },
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

    /// LFO with the given waveform and a random initial phase.
    pub fn lfo(wave: LfoWave, rate_hz: f32, depth: f32, rng: &mut SplitMix64) -> Self {
        Self::Lfo {
            depth: depth.clamp(0.0, 1.0),
            rate_hz: rate_hz.abs(),
            phase: rng.range(0.0, std::f32::consts::TAU),
            wave,
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
            Self::Lfo {
                depth,
                rate_hz,
                phase,
                wave,
            } => {
                *phase += *rate_hz * std::f32::consts::TAU * dt;
                let t = (*phase / std::f32::consts::TAU).fract();
                let v = match wave {
                    LfoWave::Sine => sin_approx(*phase),
                    LfoWave::Triangle => 4.0 * (t - 0.5).abs() - 1.0,
                    LfoWave::Saw => t * 2.0 - 1.0,
                    LfoWave::Square => {
                        if sin_approx(*phase) >= 0.0 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                };
                *depth * v
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
