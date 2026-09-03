//! History resizing: the delay line follows the history_len parameters
//! (ms when sync off, beats when on), rebuilds in place, and the freshest
//! tail survives a shrink.

use i_am_dsp::{Effect, ProcessContext};
use particula::ParticulaEngine;

const SR: usize = 48_000;

fn run(engine: &mut ParticulaEngine, n: usize, input: f32) {
    let mut ctx: Box<dyn ProcessContext> = Box::new(());
    let mut buf = [input];
    for _ in 0..n {
        engine.process(&mut buf, &[], &mut ctx);
    }
}

#[test]
fn history_resizes_ms_and_beats_and_keeps_tail() {
    let mut e = ParticulaEngine::<1>::new(512, SR, 5);
    e.texture_blend = 0.0;
    e.dry = 0.0;
    e.wet = 1.0;
    e.spawn_sync = false;
    assert_eq!(e.history_capacity_for_test(), 512);

    // Fill the delay line with 1.0 (freshest stays 1.0), then grow.
    run(&mut e, 200, 1.0);
    e.history_len_ms = 200.0; // 200ms @48k = 9600 -> pow2 16384
    run(&mut e, 4, 1.0);
    let cap = e.history_capacity_for_test();
    assert_eq!(cap, 16384, "ms-driven resize target");

    // Beat-driven target (sync on): 2 beats @ 120 BPM wildcard -> 1 s = 48000
    // -> pow2 65536.
    e.spawn_sync = true;
    e.fallback_bpm = 120.0;
    e.history_len_beats = 2.0;
    run(&mut e, 4, 0.0);
    assert_eq!(e.history_capacity_for_test(), 65536, "beats-driven resize");

    // Shrink back: freshest tail (all 1.0 from the fill) must still read 1.0
    // through a particle right after a small run that pushes more 1.0s.
    e.spawn_sync = false;
    e.history_len_ms = 30.0; // 1440 -> pow2 2048
    run(&mut e, 4, 1.0);
    assert_eq!(e.history_capacity_for_test(), 2048, "shrink target");
}
