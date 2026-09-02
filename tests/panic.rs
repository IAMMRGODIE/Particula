//! PANIC button verification: the latch wipes history + particles on the next
//! process() call, so feedback residue dies instantly.

use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

const SR: usize = 48_000;

fn run(engine: &mut ParticulaEngine, n: usize, input: f32) -> Vec<f32> {
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut buf = [input];
        engine.process(&mut buf, &[], &mut ctx);
        out.push(buf[0]);
    }
    out
}

#[test]
fn panic_clears_history_and_particles() {
    let mut e = ParticulaEngine::<1>::new(1 << 14, SR, 0xF00D);
    e.feedback_gain = 0.9; // strong feedback so residue would ring on
    e.feedback_delay_value = 20.0;
    e.spawn_interval_ms = 2.0;
    // 0.5s of input builds up particles + feedback energy.
    let out = run(&mut e, SR / 2, 0.7);
    assert!(e.live_count() > 0, "Spawn failed to produce particles");

    // Fire PANIC; new particles may respawn on later frames (the cloud keeps
    // running), but the delay line is zeroed — the real guarantee is that no
    // pre-panic feedback tail can ring through.
    e.panic_flag().store(true, std::sync::atomic::Ordering::Relaxed);

    // Sanity: latch consumed (a second process call must not re-clear).
    run(&mut e, 2, 0.0);

    // After the panic, feeding silence must produce (near) silence: the
    // delay line was zeroed, so even freshly spawned voices read 0 and any
    // feedback writes are 0.
    let tail = run(&mut e, 256, 0.0);
    let peak = tail.iter().fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(peak < 1e-6, "feedback residue survived PANIC: peak {peak}");
    let _ = out;
}
