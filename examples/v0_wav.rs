//! v0 offline demo: feed a small synthesized phrase through the engine and
//! write a float32 mono WAV so you can listen immediately.
//!
//! Run: cargo run --release --example v0_wav
//! Writes: particula_v0.wav in the project root.

use i_am_dsp::{Effect, ProcessContext};
use particula::{ParticulaEngine, SplitMix64};

const SR: usize = 48_000;
const SECONDS: usize = 4;

/// A tiny bit of test input with some sustain: two detuned saws + tremolo.
fn generate_input(seconds: usize) -> Vec<f32> {
    let n = SR * seconds;
    let mut rng = SplitMix64::new(777);
    let mut out = Vec::with_capacity(n);
    let f1 = 110.0_f32;
    let f2 = 110.0_f32 * 1.006;
    let mut ph1 = 0.0_f32;
    let mut ph2 = 0.0_f32;
    for i in 0..n {
        let t = i as f32 / SR as f32;
        ph1 += f1 / SR as f32;
        ph2 += f2 / SR as f32;
        let saw1 = 2.0 * (ph1 % 1.0) - 1.0;
        let saw2 = 2.0 * (ph2 % 1.0) - 1.0;
        let env = 0.6 + 0.4 * (2.0 * std::f32::consts::PI * 0.13 * t).sin();
        let noise = rng.sym() * 0.02;
        out.push(env * 0.2 * (saw1 + saw2) + noise);
    }
    out
}

fn write_wav_f32(path: &str, sample_rate: u32, samples: &[f32]) -> std::io::Result<()> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let byte_rate = sample_rate * 4;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    header.extend_from_slice(&1u16.to_le_bytes()); // mono
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&4u16.to_le_bytes()); // block align = 1ch * 4B
    header.extend_from_slice(&32u16.to_le_bytes()); // bits per sample
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    let mut full = header;
    full.extend_from_slice(&data);
    std::fs::write(path, full)
}

fn main() -> std::io::Result<()> {
    let input = generate_input(SECONDS);

    let mut engine = ParticulaEngine::new(1 << 15, SR, 0xC0FFEE);
    engine.dry = 0.35;
    engine.wet = 0.9;
    engine.max_particles = 96.0;
    engine.spawn_interval_ms = 45.0;
    engine.base_position = 0.45;
    engine.position_step = 0.0;
    engine.position_jitter = 0.03;
    engine.gain_decay_ratio = 0.88;
    engine.initial_gain = 0.5;
    engine.attack_ms = 8.0;
    engine.lifetime_ms_min = 120.0;
    engine.lifetime_ms_max = 1400.0;
    engine.pitch_min = 0.5;
    engine.pitch_max = 1.6;
    engine.freq_shift_min = -90.0;
    engine.freq_shift_max = 90.0;
    engine.position_smooth_ms = 25.0;
    engine.position_mode = 1; // LFO
    engine.lfo_rate_hz = 0.12;
    engine.lfo_depth = 0.18;

    // (release-grade) per-sample processing
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut out = Vec::with_capacity(input.len());
    for s in input {
        let mut buf = [s];
        engine.process(&mut buf, &[], &mut ctx);
        out.push(buf[0]);
    }

    let peak = out.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    let rms = (out.iter().map(|&s| s * s).sum::<f32>() / out.len() as f32).sqrt();

    write_wav_f32("particula_v0.wav", SR as u32, &out)?;

    println!("written particula_v0.wav");
    println!("  samples: {}", out.len());
    println!("  peak:    {peak:.3}");
    println!("  rms:     {rms:.4}");
    println!("  spawned: {}", engine.spawned());
    println!(
        "  live at end: {} (max pool {})",
        engine.live_count(),
        particula::engine::DEFAULT_POOL_CAPACITY
    );
    Ok(())
}
