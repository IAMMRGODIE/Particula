//! High-density particle load benchmark: makes sure the engine stays within a
//! few times real time even with a fat pool (384 voices), and prints the
//! actual load factor so future optimizations have a number to beat.
//!
//! Run in release for meaningful numbers:
//!   cargo test --release --test high_density --offline -- --nocapture

use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

#[test]
fn high_density_pool_stays_cheap() {
    let sr = 48_000;
    let mut e = ParticulaEngine::<2>::new(1 << 16, sr, 0xABCD);
    e.max_particles = 384.0;
    e.spawn_interval_ms = 2.0;
    e.lifetime_ms_min = 60.0;
    e.lifetime_ms_max = 600.0;
    e.texture_blend = 0.4;
    e.position_mode = 2; // RandomWalk exercises the per-sample rng path
    let frames = sr * 2; // 2 seconds of audio
    let mut in_l = vec![0.0_f32; frames];
    let mut in_r = vec![0.0_f32; frames];
    for (i, (l, r)) in in_l.iter_mut().zip(in_r.iter_mut()).enumerate() {
        let s = 0.35
            + 0.1 * (2.0 * std::f32::consts::PI * 180.0 * i as f32 / sr as f32).sin();
        *l = s;
        *r = s * 0.9;
    }

    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let start = std::time::Instant::now();
    for i in 0..frames {
        let mut buf = [in_l[i], in_r[i]];
        e.process(&mut buf, &[], &mut ctx);
        in_l[i] = buf[0];
        in_r[i] = buf[1];
    }
    let spent = start.elapsed().as_secs_f64();
    let real_time = frames as f64 / sr as f64;
    let load = spent / real_time;
    println!(
        "HIGH_DENSITY: {} frames in {:.3}s audio vs {:.3}s wall => load {:.2}x (realtime = 1.0)",
        frames, real_time, spent, load
    );
    // Generous bound: catching pathological regressions, not micro-platform
    // noise. Release-time load should be well under 1x already.
    assert!(load < 8.0, "engine too slow at high density: {load:.2}x");
    let _ = (in_l[0], in_r[0]);
}
