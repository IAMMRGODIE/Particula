//! Spawn-notifier channel + master bypass tests.

use crossbeam_channel::unbounded;
use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

const SR: usize = 48_000;

#[test]
fn spawn_notifier_emits_one_event_per_particle() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 7);
    e.texture_blend = 0.0;
    e.max_particles = 32.0;
    e.spawn_interval_ms = 20.0;
    e.lifetime_ms_min = 200.0;
    e.lifetime_ms_max = 800.0;
    let (tx, rx) = unbounded();
    e.set_spawn_notifier(tx);

    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    for i in 0..(2 * SR) {
        let t = i as f32 / SR as f32;
        let mut buf = [(2.0 * std::f32::consts::PI * 110.0 * t).sin() * 0.5];
        e.process(&mut buf, &[], &mut ctx);
    }

    let events: Vec<_> = rx.try_iter().collect();
    let spawned = e.spawned();
    assert_eq!(events.len(), spawned, "one event per spawn, events={} spawned={}", events.len(), spawned);
    // Every reported lifetime falls inside the configured range.
    let lmin = (200.0 * SR as f32 / 1000.0) as usize;
    let lmax = (800.0 * SR as f32 / 1000.0) as usize;
    for ev in &events {
        assert!((lmin..=lmax).contains(&ev.lifetime_samples), "lifetime {:?}", ev);
        assert!(ev.live <= 32);
    }
    assert!(!events.is_empty());
}

#[test]
fn enabled_off_is_straight_passthrough() {
    let mut e = ParticulaEngine::<1>::new(1024, SR, 3);
    e.enabled = false;
    let input: Vec<f32> = (0..SR)
        .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR as f32).sin())
        .collect();
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut out = Vec::with_capacity(input.len());
    for &s in &input {
        let mut buf = [s];
        e.process(&mut buf, &[], &mut ctx);
        out.push(buf[0]);
    }
    for (a, b) in input.iter().zip(out.iter()) {
        assert_eq!(a, b, "bypassed output must equal the input");
    }
    assert_eq!(e.spawned(), 0, "no processing while disabled");
}