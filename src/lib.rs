//! Particula — experimental sound-design particle effecter.
//!
//! v0 scope: a mono granular-cloud engine. Particles read from a shared
//! history ring buffer at smoothed t-space positions, apply playback rate
//! (granular pitch), per-particle IIR-Hilbert frequency shift, and an
//! exponential-decay envelope, then sum into the output.
//!
//! Not yet in v0 (see Architecture.md): feedback writes, WSOLA texture layer,
//! BPM sync, stereo, CLAP wrapper.

pub mod engine;
pub mod history;
pub mod particle;
pub mod position_mod;
pub mod rng;
pub mod spawner;

pub use engine::ParticulaEngine;
pub use history::{add_at, recent_peak_position};
pub use particle::Particle;
pub use position_mod::PositionMod;
pub use rng::SplitMix64;
pub use spawner::{Spawn, Spawner};
