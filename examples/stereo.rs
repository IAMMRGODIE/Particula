//! Stereo demo: shared-history particle cloud with per-particle pan spread,
//! feedback + peak-follow positions + WSOLA texture. Writes a stereo
//! particula_stereo.wav.
//!
//! Run: cargo run --release --offline --example stereo

use i_am_dsp::{Effect, ProcessContext};
use particula::{ParticulaEngine, SplitMix64};

const SR: usize = 48_000;
const SECONDS: usize = 6;

/// Slightly detuned stereo saws: a stable L/R bed for the cloud to chew.
fn generate_input(seconds: usize) -> (Vec<f32>, Vec<f32>) {
    let n = SR * seconds;
    let mut rng = SplitMix64::new(2024);
    let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut ph = [0.0_f32; 4];
    let freqs = [110.0_f32, 110.0 * 1.007, 330.0, 330.0 * 1.004];
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let mut frame = [0.0_f32; 2];
        for (k, &f) in freqs.iter().enumerate() {
            ph[k] += f / SR as f32;
            let saw = 2.0 * (ph[k] % 1.0) - 1.0;
            frame[k % 2] += saw * 0.11;
        }
        // gentle stereo width modulation + noise floor
        let w = 0.85 + 0.15 * (2.0 * std::f32::consts::PI * 0.07 * t).sin();
        let noise = rng.sym() * 0.01;
        l.push(frame[0] * w + noise);
        r.push(frame[1] * (2.0 - w) + noise * 0.5);
    }
    (l, r)
}

fn write_stereo_wav_f32(path: &str, sample_rate: u32, l: &[f32], r: &[f32]) -> std::io::Result<()> {
    let mut data = Vec::with_capacity(l.len() * 8);
    for (a, b) in l.iter().zip(r.iter()) {
        data.extend_from_slice(&a.to_le_bytes());
        data.extend_from_slice(&b.to_le_bytes());
    }
    let byte_rate = sample_rate * 8;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    header.extend_from_slice(&2u16.to_le_bytes()); // stereo
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&8u16.to_le_bytes()); // block align = 2ch * 4B
    header.extend_from_slice(&32u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    let mut full = header;
    full.extend_from_slice(&data);
    std::fs::write(path, full)
}

fn main() -> std::io::Result<()> {
    let (input_l, input_r) = generate_input(SECONDS);

    let mut engine = ParticulaEngine::<2>::new(1 << 16, SR, 0x57E2E0);
    engine.dry = 0.30;
    engine.wet = 1.0;
    engine.max_particles = 96.0;
    engine.spawn_interval_ms = 40.0;
    engine.base_position = 0.8;
    engine.position_jitter = 0.05;
    engine.gain_decay_ratio = 0.9;
    engine.initial_gain = 0.5;
    engine.attack_ms = 6.0;
    engine.lifetime_ms_min = 120.0;
    engine.lifetime_ms_max = 1100.0;
    engine.pitch_min = 0.5;
    engine.pitch_max = 1.6;
    engine.freq_shift_min = -70.0;
    engine.freq_shift_max = 70.0;
    engine.position_smooth_ms = 18.0;
    engine.position_mode = 3; // PeakFollow
    engine.peak_window_ms = 100.0;
    engine.peak_update_ms = 25.0;
    engine.peak_threshold = 0.004;
    engine.feedback_gain = 0.4;
    engine.feedback_delay_value = 25.0;
    engine.feedback_damping_hz = 2500.0;
    engine.texture_blend = 0.55;
    engine.texture_window_ms = 110.0;
    engine.texture_refresh_ms = 55.0;
    engine.texture_stretch = 0.85;
    engine.texture_crossfade_ms = 15.0;
    engine.pan_min = -1.0; // full stereo spread
    engine.pan_max = 1.0;
    engine.spawn_sync = true;
    engine.spawn_interval_beats = 0.25;
    engine.fallback_bpm = 120.0;

    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let (mut out_l, mut out_r) = (Vec::with_capacity(input_l.len()), Vec::with_capacity(input_r.len()));
    for (l, r) in input_l.into_iter().zip(input_r) {
        let mut buf = [l, r];
        engine.process(&mut buf, &[], &mut ctx);
        out_l.push(buf[0]);
        out_r.push(buf[1]);
    }

    let peak_l = out_l.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    let peak_r = out_r.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    let rms_l = (out_l.iter().map(|&s| s * s).sum::<f32>() / out_l.len() as f32).sqrt();
    let rms_r = (out_r.iter().map(|&s| s * s).sum::<f32>() / out_r.len() as f32).sqrt();
    write_stereo_wav_f32("particula_stereo.wav", SR as u32, &out_l, &out_r)?;

    println!("written particula_stereo.wav");
    println!("  samples: {}", out_l.len());
    println!("  peak L/R: {peak_l:.3} / {peak_r:.3}");
    println!("  rms  L/R: {rms_l:.4} / {rms_r:.4}");
    println!("  spawned: {}", engine.spawned());
    Ok(())
}