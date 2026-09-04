//! Read-path pitch regression: with a 440 Hz sine filling the (pushed) line,
//! rate +1 and rate -1 must both read back at ~440 Hz (original pitch).
use std::f32::consts::TAU;
use i_am_dsp::{ProcessContext, tools::ring_buffer::RingBuffer};
use particula::{Particle, PositionMod, SplitMix64, Texture};
const SR: usize = 48_000;
const CAP: usize = 1 << 12;
const F0: f32 = 440.0;

fn freq(rate: f32) -> f32 {
    let mut history = RingBuffer::<f32>::new(CAP);
    // Pre-roll: fill the line with 440 sine so reverse reads have material.
    for j in 0..CAP {
        history.push((TAU * F0 * (j as isize - CAP as isize) as f32 / SR as f32).sin());
    }
    let texture = Texture::new(512, SR);
    let mut p = Particle::new(SR, 0.42, PositionMod::fixed(), rate, 0.0, 1.0, 0.0, 0.0, 1, 20000, 0.1);
    let mut rng = SplitMix64::new(9);
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let to = if rate < 0.0 { 1000 } else { 20000 };
    let from = if rate < 0.0 { 50 } else { 2048 };
    let mut prev = 0.0_f32;
    let mut zc = 0usize;
    for i in 0..to {
        history.push((TAU * F0 * i as f32 / SR as f32).sin());
        let s = p.process(&mut history, &texture, 0.0, 1.0 / SR as f32, i, 0.5, 0, 1.0, &mut rng, &mut ctx).unwrap_or(0.0);
        if i >= from && prev <= 0.0 && s > 0.0 {
            zc += 1;
        }
        prev = s;
    }
    zc as f32 / ((to - from) as f32 / SR as f32)
}

#[test]
fn read_path_pitch_preserved() {
    for rate in [1.0_f32, -1.0] {
        let f = freq(rate);
        println!("read-path rate {rate:+} -> {f:.0} Hz");
        assert!(
            (f - F0).abs() < F0 * 0.08,
            "rate {rate} should read near {F0} Hz, got {f}"
        );
    }
}
