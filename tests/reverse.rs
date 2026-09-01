//! Reverse playback: particles with a negative playback rate must walk the
//! read head towards older history samples (backwards through material).

use i_am_dsp::{Effect, ProcessContext, tools::ring_buffer::RingBuffer};
use particula::{ParticulaEngine, Particle, PositionMod, SplitMix64, Texture};

const SR: usize = 48_000;

/// One sample of a fixed-position particle with a known rate; returns the
/// read position after each processed sample.
fn track_positions(rate: f32, frames: usize) -> Vec<f32> {
    let mut history = RingBuffer::<f32>::new(256);
    for _ in 0..256 {
        history.push(1.0_f32); // DC: interpolation is identity everywhere
    }
    let texture = Texture::new(64, SR);
    let mut particle = Particle::new(
        SR,
        0.8,        // onset
        PositionMod::fixed(),
        rate,
        0.0,        // freq shift
        1.0,        // initial gain
        0.0,        // feedback
        0.0,        // pan
        1,          // attack samples
        48_000,     // lifetime
        1.0,        // smooth ms
    );
    let mut rng = SplitMix64::new(7);
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        out.push(particle.position);
        particle.process(
            &mut history,
            &texture,
            0.0,               // texture blend
            1.0 / SR as f32,
            i,
            0.8,               // peak_t (unused: Fixed)
            0,                 // feedback delay
            1.0,               // feedback lp a
            &mut rng,
            &mut ctx,
        );
    }
    out
}

#[test]
fn forward_rate_walks_towards_newer_history() {
    let pos = track_positions(1.0, 500);
    // Fixed onset + positive rate: position increases by 1/cap per sample.
    let step = pos[1] - pos[0];
    assert!((step - 1.0 / 256.0).abs() < 1e-6, "forward step {step}");
}

#[test]
fn reverse_rate_walks_towards_older_history() {
    let pos = track_positions(-1.0, 500);
    let mut wraps = 0;
    for w in pos.windows(2) {
        let diff = w[1] - w[0];
        if diff > 0.5 {
            // rem_euclid wrap from t ~ 0 to t ~ 1
            wraps += 1;
        } else {
            assert!(diff < 0.0, "reverse position must strictly decrease, got {diff}");
            assert!(
                (diff + 1.0 / 256.0).abs() < 1e-6,
                "reverse step {diff} should be -1/cap"
            );
        }
    }
    assert!(wraps > 0, "500 samples at -1/256 must wrap several times");
}

#[test]
fn reverse_chance_changes_the_audio() {
    let input: Vec<f32> = (0..SR)
        .map(|i| {
            let t = i as f32 / SR as f32;
            // A transient-rich signal makes direction audible.
            if (i % 12_000) < 400 {
                0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            } else {
                0.05
            }
        })
        .collect();

    let run_cloud = |chance: f32| -> f32 {
        let mut e = ParticulaEngine::<1>::new(4096, SR, 123);
        e.texture_blend = 0.0;
        e.reverse_chance = chance;
        e.pitch_min = 0.9;
        e.pitch_max = 1.1;
        e.max_particles = 32.0;
        e.spawn_interval_ms = 15.0;
        e.base_position = 0.85;
        e.lifetime_ms_min = 300.0;
        e.lifetime_ms_max = 600.0;
        let mut ctx: Box<dyn ProcessContext> = Box::new(());
        let mut sq = 0.0_f64;
        for s in &input {
            let mut buf = [*s];
            e.process(&mut buf, &[], &mut ctx);
            sq += buf[0] as f64 * buf[0] as f64;
        }
        (sq / input.len() as f64).sqrt() as f32
    };

    let fwd = run_cloud(0.0);
    let rev = run_cloud(1.0);
    assert!(fwd.is_finite() && rev.is_finite());
    assert!(
        (rev - fwd).abs() > 1e-4,
        "reverse playback must audibly change the result (fwd {fwd}, rev {rev})"
    );
}

#[test]
fn reverse_and_forward_particles_coexist() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 9);
    e.texture_blend = 0.0;
    e.reverse_chance = 0.5;
    e.max_particles = 16.0;
    e.spawn_interval_ms = 10.0;
    e.lifetime_ms_min = 100.0;
    e.lifetime_ms_max = 200.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut peak = 0.0_f32;
    for i in 0..SR {
        let t = i as f32 / SR as f32;
        let mut buf = [(2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.5];
        e.process(&mut buf, &[], &mut ctx);
        peak = peak.max(buf[0].abs());
    }
    assert!(peak.is_finite() && peak > 1e-4, "mixed directions must sound");
    let positions = e.particle_positions();
    assert!(!positions.is_empty(), "live particles expected");
    assert!(positions.iter().all(|p| (0.0..1.0).contains(p)));
}
