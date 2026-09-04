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
        history.push(1.0); // mimic the real engine: the line advances 1/frame
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
fn forward_rate_reads_at_constant_distance() {
    let pos = track_positions(1.0, 500);
    // Push-aware drift: rate 1 keeps t constant, i.e. the read head holds a
    // constant distance from the freshest sample = delayed playback at the
    // original pitch (not marching through the line).
    let step = pos[1] - pos[0];
    assert!(step.abs() < 1e-5, "forward step {step} should be ~0");
    assert!((pos[0] - 0.8).abs() < 1e-5, "starts at the onset");
}

#[test]
fn reverse_rate_walks_towards_older_history() {
    let pos = track_positions(-1.0, 500);
    // Reverse reads are a continuous read-head distance: t grows by
    // (1 - rate)/(cap-1) = 2/255 per frame — absolute sample speed -1
    // (original pitch, backwards) with no rem wrap jumps.
    for w in pos.windows(2) {
        let diff = w[1] - w[0];
        let expected = 2.0 / 255.0;
        assert!(
            (diff - expected).abs() < 1e-6,
            "reverse step {diff} should be +{expected}"
        );
    }
    // The head drifts past the buffered span and must not wrap back: it
    // eventually exceeds 1.0 (read silence), never snaps toward 0.
    assert!(
        pos[0] < pos[1] && pos[1] < pos[490],
        "reverse t should keep growing monotonically"
    );
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
