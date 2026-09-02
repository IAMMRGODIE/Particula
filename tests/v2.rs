//! v2 verification tests: WSOLA texture layer.

use i_am_dsp::{
    Effect, ProcessContext,
    tools::ring_buffer::RingBuffer,
};
use particula::{ParticulaEngine, Texture};

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

/// A history of sustained-ish audio for the texture to stretch.
fn make_history(n: usize) -> RingBuffer<f32> {
    let mut h = RingBuffer::new(n);
    for i in 0..n {
        let t = i as f32 / SR as f32;
        h.push(0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + 0.5);
    }
    h
}

#[test]
fn texture_refreshes_and_reads_finite() {
    let mut tex = Texture::new(1024, SR);
    let history = make_history(4096);
    for n in 0..4_096usize {
        tex.process(&history, 1.0, 512, 300, n);
    }
    assert!(tex.refreshes() > 0, "texture must refresh on schedule");
    for &t in &[0.0, 0.13, 0.5, 0.97] {
        let s = tex.sample(t);
        assert!(s.is_finite(), "texture sample not finite at t={t}: {s}");
        assert!(s.abs() <= 1.5, "texture level implausible at t={t}: {s}");
    }
}

#[test]
fn texture_dc_input_stays_dc() {
    // WSOLA of a DC window with stretch 1.0 stays (approximately) DC.
    let mut tex = Texture::new(512, SR);
    let mut history = RingBuffer::new(2048);
    for _ in 0..2048 {
        history.push(1.0_f32);
    }
    for n in 0..1_000usize {
        tex.process(&history, 1.0, 300, 100, n);
    }
    assert!(tex.refreshes() >= 1);
    let s = tex.sample(0.5);
    assert!(
        (s - 1.0).abs() < 0.25,
        "DC stretches to DC, got {s}"
    );
}

#[test]
fn texture_stretch_change_and_crossfade_are_safe() {
    let mut tex = Texture::new(1024, SR);
    let history = make_history(4096);
    let mut stretch = 1.0_f32;
    for n in 0..6_000usize {
        if n == 2_000 {
            stretch = 0.5; // abrupt param change mid-stream
        }
        if n == 4_000 {
            stretch = 2.0;
        }
        tex.process(&history, stretch, 400, 200, n);
    }
    for &t in &[0.1, 0.5, 0.9] {
        let s = tex.sample(t);
        assert!(s.is_finite(), "sample not finite at t={t}: {s}");
    }
}

#[test]
fn texture_blend_changes_output() {
    let input: Vec<f32> = (0..SR)
        .map(|i| {
            let t = i as f32 / SR as f32;
            0.4 * (2.0 * std::f32::consts::PI * 110.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect();

    let mut plain = ParticulaEngine::<1>::new(4096, SR, 21);
    plain.dry = 0.4;
    plain.wet = 1.0;
    plain.texture_blend = 0.0;
    plain.base_position = 0.8;
    plain.position_jitter = 0.1;
    let out_plain = run(&mut plain, &input);

    let mut textured = ParticulaEngine::<1>::new(4096, SR, 21);
    textured.dry = 0.4;
    textured.wet = 1.0;
    textured.texture_blend = 0.8;
    textured.texture_window_ms = 100.0;
    textured.texture_refresh_ms = 50.0;
    textured.texture_stretch = 0.75;
    textured.base_position = 0.8;
    textured.position_jitter = 0.1;
    let out_textured = run(&mut textured, &input);

    for &s in out_textured.iter() {
        assert!(s.is_finite(), "non-finite with texture: {s}");
    }
    let rms = |v: &[f32]| (v.iter().map(|&s| s * s).sum::<f32>() / v.len() as f32).sqrt();
    assert!(
        (rms(&out_textured) - rms(&out_plain)).abs() > 1e-4,
        "texture blend should change the output (tex={} plain={})",
        rms(&out_textured),
        rms(&out_plain)
    );
}

#[test]
fn texture_with_feedback_stays_bounded() {
    let mut e = ParticulaEngine::<1>::new(8192, SR, 33);
    e.dry = 0.3;
    e.wet = 1.0;
    e.texture_blend = 0.6;
    e.texture_stretch = 0.8;
    e.feedback_gain = 0.5;
    e.feedback_delay_value = 12.0;
    e.feedback_damping_hz = 3000.0;
    e.base_position = 0.9;
    e.position_jitter = 0.07;
    e.max_particles = 48.0;
    let input: Vec<f32> = (0..SR)
        .map(|i| 0.5_f32 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR as f32).sin())
        .collect();
    let out = run(&mut e, &input);
    let peak = out.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(peak < 4.0, "texture+feedback must stay bounded, peak {peak}");
    for &s in &out {
        assert!(s.is_finite());
    }
}