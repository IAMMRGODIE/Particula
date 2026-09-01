//! Long-running survival: the generational strength decay must not let the
//! cloud exhaust itself into silence a few seconds after input starts.

use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

const SR: usize = 48_000;
const RUN_SECONDS: usize = 15;

#[test]
fn default_cloud_keeps_spawning_and_stays_audible() {
    let mut e = ParticulaEngine::<1>::new(8192, SR, 7);
    // Intentionally all defaults (including gain_decay_ratio 0.9): without
    // the min_gain_ratio floor every particle after ~110 generations is born
    // below the 1e-5 survival threshold and the cloud dies at ~3.3 s.
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let total = RUN_SECONDS * SR;
    let mut tail_sq = 0.0_f64;
    for i in 0..total {
        let t = i as f32 / SR as f32;
        let input = 0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
        let mut buf = [input];
        e.process(&mut buf, &[], &mut ctx);
        if i >= (RUN_SECONDS - 1) * SR {
            tail_sq += buf[0] as f64 * buf[0] as f64;
        }
    }

    let tail_rms = (tail_sq / SR as f64).sqrt() as f32;
    // Steady state sits at the -26 dB floor: clearly audible but quieter
    // than the opening burst.
    assert!(
        tail_rms > 5e-3,
        "cloud went silent in the final second: rms {tail_rms}"
    );
    assert!(e.live_count() > 0, "no live particles at the end");
    assert!(
        e.spawned() > 200,
        "spawning should continue far past the pool limit: {}",
        e.spawned()
    );
}

#[test]
fn spawn_rule_gain_bottoms_out_at_floor() {
    let mut e = ParticulaEngine::<1>::new(512, SR, 1);
    e.initial_gain = 1.0;
    e.gain_decay_ratio = 0.9;
    e.min_gain_ratio = 0.05;
    let floor = e.spawn_rule_gain(10_000);
    assert!(
        (floor - e.initial_gain * e.min_gain_ratio).abs() < 1e-7,
        "floor hold: {floor}"
    );
    assert!(e.spawn_rule_gain(5) > e.spawn_rule_gain(50), "early decay alive");
}