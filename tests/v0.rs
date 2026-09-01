//! v0 verification tests: RNG determinism, spawn rule, read point + envelope,
//! lifecycle/death.

use i_am_dsp::{Effect, ProcessContext};
use particula::{ParticulaEngine, SplitMix64};

const SR: usize = 48_000;

/// Drives the engine over a whole buffer with a default (empty) context.
fn run(engine: &mut ParticulaEngine, input: &[f32]) -> Vec<f32> {
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    input
        .iter()
        .map(|&s| {
            let mut buf = [s];
            engine.process(&mut buf, &[], &mut ctx);
            buf[0]
        })
        .collect()
}

#[test]
fn rng_is_deterministic_and_in_range() {
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    for _ in 0..64 {
        assert_eq!(a.next_u64(), b.next_u64());
    }

    let mut r = SplitMix64::new(7);
    for _ in 0..100_000 {
        let x = r.next_f32();
        assert!((0.0..1.0).contains(&x), "next_f32 out of range: {x}");
        let s = r.sym();
        assert!((-1.0..1.0).contains(&s), "sym out of range: {s}");
        let u = r.range_usize(10, 20);
        assert!((10..=20).contains(&u), "range_usize out of range: {u}");
    }
}

#[test]
fn spawn_rule_is_arithmetic_with_exponential_strength_decay() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 1);
    e.base_position = 0.2;
    e.position_step = 0.1;
    e.initial_gain = 1.0;
    e.gain_decay_ratio = 0.9;
    e.position_jitter = 0.0;

    assert!((e.spawn_rule_position(0) - 0.2).abs() < 1e-6);
    assert!((e.spawn_rule_position(1) - 0.3).abs() < 1e-6);
    assert!((e.spawn_rule_position(2) - 0.4).abs() < 1e-6);
    // wraps via rem_euclid
    e.base_position = 0.95;
    assert!((e.spawn_rule_position(1) - 0.05).abs() < 1e-6);
    e.base_position = 0.2;

    assert!((e.spawn_rule_gain(0) - 1.0).abs() < 1e-6);
    assert!((e.spawn_rule_gain(1) - 0.9).abs() < 1e-6);
    assert!((e.spawn_rule_gain(5) - 0.9f32.powi(5)).abs() < 1e-6);
}

/// DC input + a single fixed-reading particle: once the whole history is
/// filled, every read point is DC (cubic interpolation of constants is
/// identity, shift 0 passes through), so output == the envelope gain.
///
/// Read position t=0 is the oldest sample and t=1 the freshest, so with a
/// 1024-sample history the fill time is ~1024 samples; we check after that.
#[test]
fn dc_input_reads_position_and_envelope() {
    let mut e = ParticulaEngine::<1>::new(1024, SR, 99);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0; // pure history read test
    e.position_mode = 0; // fixed
    e.base_position = 0.25;
    e.position_step = 0.0;
    e.position_jitter = 0.0;
    e.pitch_min = 0.0;
    e.pitch_max = 0.0; // no drift
    e.freq_shift_min = 0.0;
    e.freq_shift_max = 0.0;
    e.initial_gain = 0.5;
    e.gain_decay_ratio = 1.0; // no generation decay
    e.max_particles = 1.0;
    e.spawn_interval_ms = 1.0; // spawn immediately, then pool is full
    e.attack_ms = 1.0;
    e.lifetime_ms_min = 10_000.0;
    e.lifetime_ms_max = 10_000.0;
    e.position_smooth_ms = 1.0;

    let input = vec![1.0; 2_500];
    let out = run(&mut e, &input);

    for &s in &out {
        assert!(s.is_finite(), "non-finite output: {s}");
    }
    // History fully written by ~1024 + 48 attack samples; steady value = 0.5.
    let steady = out[1_500];
    assert!(
        (steady - 0.5).abs() < 0.03,
        "expected ~0.5 envelope gain, got {steady}"
    );
    // Envelope decays monotonically after the attack peak.
    assert!(out[2_200] < out[1_600], "envelope must decay");
    assert!(out[2_200] > 0.0, "envelope should not hit zero this early");
}

/// A single particle born once must read real audio, then die and leave total
/// silence afterwards (where "silence" includes dry, so dry=0 here).
#[test]
fn single_particle_dies_and_output_goes_silent() {
    let mut e = ParticulaEngine::<1>::new(512, SR, 3);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0; // pure history read test
    e.position_mode = 0;
    e.base_position = 0.75; // near the freshest end: readable soon after fill
    e.position_jitter = 0.0;
    e.pitch_min = 0.0;
    e.pitch_max = 0.0;
    e.freq_shift_min = 0.0;
    e.freq_shift_max = 0.0;
    e.initial_gain = 1.0;
    e.gain_decay_ratio = 1.0;
    e.max_particles = 1.0;
    e.spawn_interval_ms = 10_000.0; // one spawn, then never again
    e.lifetime_ms_min = 50.0;
    e.lifetime_ms_max = 50.0; // 2400 samples
    e.position_smooth_ms = 5.0;

    let input = vec![1.0; 3_500]; // DC input: wet reads 1.0 * envelope
    let out = run(&mut e, &input);

    assert_eq!(e.spawned(), 1, "exactly one spawn expected");
    // At 800 samples (~1/3 of the 2400-sample lifetime) the exponential
    // envelope has reached about -19 dB, so the grain must still be audible.
    assert!(out[800].abs() > 0.05, "particle should be clearly audible mid-life");
    assert!(out[800] < 0.5, "mid-life gain should have decayed below peak");
    assert_eq!(e.live_count(), 0, "particle must be dead at the end");
    for &s in &out[3_000..] {
        assert!(s.abs() < 1e-6, "expected silence after death, got {s}");
    }
}

/// Default-ish configuration over non-silent input: bounded, finite, sane
/// live count.
#[test]
fn default_cloud_stays_bounded_and_finite() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 1234);
    let mut rng = SplitMix64::new(5);
    let n = SR; // 1 s
    let input: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.5 + rng.sym() * 0.01
        })
        .collect();
    let out = run(&mut e, &input);
    for &s in &out {
        assert!(s.is_finite(), "non-finite output: {s}");
    }
    assert!(e.live_count() <= 256);
    assert!(e.spawned() > 0, "cloud must spawn");
    // Bounded by (64 particles * initial gain 0.5) + dry input.
    let peak = out.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
    assert!(peak < 64.0 * 0.5 + 1.0, "unexpectedly hot output: {peak}");
}
