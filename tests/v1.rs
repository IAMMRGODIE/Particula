//! v1 verification tests: history add_at, peak follower, feedback behaviour.

use i_am_dsp::{
    Effect, ProcessContext,
    tools::ring_buffer::RingBuffer,
};
use particula::ParticulaEngine;

const SR: usize = 48_000;

/// Drives the engine over a whole buffer with a default context.
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
fn add_at_writes_behind_the_freshest_sample() {
    let mut h = RingBuffer::new(16);
    for v in [1.0_f32, 2.0, 3.0, 4.0, 5.0] {
        h.push(v);
    }
    // After 5 pushes the freshest slot (index capacity-1) holds 5.0.
    assert_eq!(h[15], 5.0_f32);

    // delay 0 -> exactly the freshest slot (the pseudocode history[-1]).
    particula::add_at(&mut h, 0, 10.0);
    assert_eq!(h[15], 15.0_f32);

    // delay 2 -> two slots behind the freshest: the 3.0 value.
    particula::add_at(&mut h, 2, 20.0);
    assert_eq!(h[13], 23.0_f32);
    assert_eq!(h[15], 15.0_f32, "freshest untouched by delayed write");

    // Oversized delay clamps to the oldest slot (delay = cap-1) without
    // panicking; the freshest slot stays untouched.
    particula::add_at(&mut h, 64, 5.0);
    assert_eq!(h[0], 5.0_f32);
    assert_eq!(h[15], 15.0_f32);
}

#[test]
fn recent_peak_finds_the_loudest_recent_sample() {
    // Fill 64 samples: 60 quiet, then one loud 0.9, then 3 quiet.
    let mut h = RingBuffer::new(64);
    for _ in 0..60 {
        h.push(0.05);
    }
    h.push(0.9);
    for _ in 0..3 {
        h.push(0.05);
    }
    // 60 quiet pushes fill slots 0..59, then 0.9 lands in slot 60,
    // which is head-relative 60 -> t = 60 / 63.
    let t = particula::recent_peak_position(&h, 64, 0.01);
    assert!((t - 60.0 / 63.0).abs() < 1e-4, "peak t = {t}");

    // Threshold above the loudest -> fallback to the freshest position (1.0).
    let t2 = particula::recent_peak_position(&h, 64, 1.0);
    assert!((t2 - 1.0).abs() < 1e-6, "fallback t = {t2}");
}

#[test]
fn feedback_changes_output_and_stays_bounded() {
    let input: Vec<f32> = (0..SR)
        .map(|i| {
            let t = i as f32 / SR as f32;
            // A transient-rich phrase: pulses every 250 ms.
            let pulse = if (i % 12_000) < 800 { 1.0 } else { 0.05 };
            pulse * (2.0 * std::f32::consts::PI * 110.0 * t).sin()
        })
        .collect();

    // Off version.
    let mut off = ParticulaEngine::<1>::new(4096, SR, 11);
    off.dry = 0.4;
    off.wet = 1.0;
    off.texture_blend = 0.0; // isolate the feedback path
    off.feedback_gain = 0.0;
    off.base_position = 0.9;
    off.position_jitter = 0.05;
    off.lifetime_ms_min = 150.0;
    off.lifetime_ms_max = 600.0;
    let out_off = run(&mut off, &input);

    // On version (feedback loop at 60 ms, damped at 4 kHz).
    let mut on = ParticulaEngine::<1>::new(4096, SR, 11);
    on.dry = 0.4;
    on.wet = 1.0;
    on.texture_blend = 0.0; // isolate the feedback path
    on.feedback_gain = 0.7;
    // Keep the injection point inside the particles' read region
    // (base 0.9 ± jitter): delay 8 ms -> h = 4095 - 384 = 3711, well inside.
    on.feedback_delay_ms = 8.0;
    on.feedback_damping_hz = 4000.0;
    on.base_position = 0.9;
    on.position_jitter = 0.08;
    on.lifetime_ms_min = 150.0;
    on.lifetime_ms_max = 600.0;
    let out_on = run(&mut on, &input);

    for &s in out_on.iter() {
        assert!(s.is_finite(), "non-finite with feedback: {s}");
    }
    let rms = |v: &[f32]| (v.iter().map(|&s| s * s).sum::<f32>() / v.len() as f32).sqrt();
    let peak_on = out_on.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(peak_on < 4.0, "feedback must stay bounded, peak {peak_on}");
    assert!(
        (rms(&out_on) - rms(&out_off)).abs() > 1e-4,
        "feedback should measurably change the output (on={} off={})",
        rms(&out_on),
        rms(&out_off)
    );
}

#[test]
fn no_self_oscillation_on_silence() {
    let mut e = ParticulaEngine::<1>::new(1024, SR, 42);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0; // isolate the feedback path
    e.feedback_gain = 0.9;
    e.feedback_delay_ms = 20.0;
    e.feedback_damping_hz = 2000.0;
    e.position_mode = 1;
    e.base_position = 0.99;
    e.position_jitter = 0.0;
    e.initial_gain = 0.8;
    e.pitch_min = 0.5;
    e.pitch_max = 1.5;
    e.lifetime_ms_min = 50.0;
    e.lifetime_ms_max = 200.0;
    let out = run(&mut e, &vec![0.0; SR]);
    let peak = out.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(peak < 1e-3, "silent input must not self-oscillate, peak {peak}");
}

#[test]
fn peak_follow_mode_runs_and_reads() {
    let mut e = ParticulaEngine::<1>::new(2048, SR, 7);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0; // isolate the peak-follow path
    e.position_mode = 3; // PeakFollow
    e.peak_window_ms = 40.0;
    e.peak_update_ms = 5.0;
    e.peak_threshold = 0.0001;
    e.max_particles = 8.0;
    e.spawn_interval_ms = 20.0;
    e.lifetime_ms_min = 200.0;
    e.lifetime_ms_max = 800.0;
    e.initial_gain = 0.6;
    e.pitch_min = 0.8;
    e.pitch_max = 1.2;
    e.freq_shift_min = -50.0;
    e.freq_shift_max = 50.0;
    // Impulsive input so the recent-window peak is unmistakable.
    let mut input = vec![0.05_f32; SR];
    for &i in &[500, 600, 700, 5000, 5020, 20_000] {
        input[i] = 0.9;
    }
    let out = run(&mut e, &input);
    for &s in &out {
        assert!(s.is_finite(), "non-finite output: {s}");
    }
    assert!(e.spawned() > 0, "cloud must spawn");
    let peak = out.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(peak > 0.01, "particles must have read something, peak {peak}");
}