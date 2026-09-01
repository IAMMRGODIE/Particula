//! v2 stereo verification: pan distribution, mono-mix history semantics.

use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

const SR: usize = 48_000;

/// Drives a stereo engine over n samples; returns (L, R).
fn run_stereo(engine: &mut ParticulaEngine<2>, n: usize, l: f32, r: f32) -> (Vec<f32>, Vec<f32>) {
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut out_l = Vec::with_capacity(n);
    let mut out_r = Vec::with_capacity(n);
    for _ in 0..n {
        let mut buf = [l, r];
        engine.process(&mut buf, &[], &mut ctx);
        out_l.push(buf[0]);
        out_r.push(buf[1]);
    }
    (out_l, out_r)
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|&s| s * s).sum::<f32>() / v.len() as f32).sqrt()
}

/// A stereo engine pinned to one pan value, DC input, history read near the
/// freshest end.
fn pan_pinned(seed: u64, pan: f32) -> ParticulaEngine<2> {
    let mut e = ParticulaEngine::<2>::new(2048, SR, seed);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0;
    e.feedback_gain = 0.0;
    e.position_mode = 0;
    e.base_position = 0.9;
    e.position_jitter = 0.0;
    e.pitch_min = 0.0;
    e.pitch_max = 0.0;
    e.freq_shift_min = 0.0;
    e.freq_shift_max = 0.0;
    e.initial_gain = 0.8;
    e.gain_decay_ratio = 1.0;
    e.max_particles = 2.0;
    e.spawn_interval_ms = 1.0;
    e.lifetime_ms_min = 100.0;
    e.lifetime_ms_max = 100.0;
    e.pan_min = pan;
    e.pan_max = pan;
    e
}

#[test]
fn stereo_hard_left_keeps_right_almost_silent() {
    let mut e = pan_pinned(1, -1.0);
    let (l, r) = run_stereo(&mut e, SR, 1.0, 1.0);
    let (rl, rr) = (rms(&l), rms(&r));
    assert!(rl > 0.2, "left channel should carry the wet signal, rms {rl}");
    assert!(rr < 1e-3, "right channel should be near-silent at hard left, rms {rr}");
}

#[test]
fn stereo_hard_right_distributes_to_right() {
    let mut e = pan_pinned(2, 1.0);
    let (l, r) = run_stereo(&mut e, SR, 1.0, 1.0);
    let (rl, rr) = (rms(&l), rms(&r));
    assert!(rr > 0.2, "right channel should carry the wet signal, rms {rr}");
    assert!(rl < 1e-3, "left channel should be near-silent at hard right, rms {rl}");
}

#[test]
fn stereo_center_pan_gives_balanced_channels() {
    let mut e = pan_pinned(3, 0.0);
    let (l, r) = run_stereo(&mut e, SR, 1.0, 1.0);
    let (rl, rr) = (rms(&l), rms(&r));
    let ratio = (rl / rr.max(1e-9)).abs();
    assert!((ratio - 1.0).abs() < 0.1, "center pan should balance, L/R = {ratio}");
}

#[test]
fn stereo_out_of_phase_input_silences_shared_history() {
    let mut e = ParticulaEngine::<2>::new(2048, SR, 4);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0;
    e.feedback_gain = 0.0;
    e.initial_gain = 0.8;
    e.base_position = 0.9;
    e.position_jitter = 0.0;
    e.pan_min = -1.0;
    e.pan_max = 1.0;
    let (l, r) = run_stereo(&mut e, SR, 1.0, -1.0);
    // L + R cancels to 0 in the mono mix -> history silent -> wet silent.
    assert!(rms(&l) < 1e-3, "history from mono mix must be silent, L rms {}", rms(&l));
    assert!(rms(&r) < 1e-3, "history from mono mix must be silent, R rms {}", rms(&r));
}

#[test]
fn stereo_default_config_stays_bounded() {
    let mut e = ParticulaEngine::<2>::new(4096, SR, 5);
    e.dry = 0.4;
    e.wet = 1.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut peak = 0.0_f32;
    for i in 0..SR {
        let t = i as f32 / SR as f32;
        let mut buf = [
            (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.5,
            (2.0 * std::f32::consts::PI * 223.0 * t).sin() * 0.5,
        ];
        e.process(&mut buf, &[], &mut ctx);
        peak = peak.max(buf[0].abs()).max(buf[1].abs());
    }
    assert!(peak.is_finite() && peak < 4.0, "stereo output bounded, peak {peak}");
}