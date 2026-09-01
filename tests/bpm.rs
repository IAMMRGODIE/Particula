//! v2 BPM sync verification: beat-quantized spawning, fallback without a
//! trusted tempo, transport-restart phase alignment.

use i_am_dsp::{Effect, NoteEvent, ProcessContext, ProcessInfos};

use particula::ParticulaEngine;

const SR: usize = 48_000;

/// Controllable process context: a static ProcessInfos snapshot.
struct TestCtx {
    infos: ProcessInfos,
}

impl TestCtx {
    fn new(tempo: Option<f32>, playing: bool, trustable: bool) -> Self {
        let mut infos = ProcessInfos::default();
        infos.sample_rate = SR;
        infos.tempo = tempo;
        infos.playing = playing;
        infos.trustable = trustable;
        Self { infos }
    }
}

impl ProcessContext for TestCtx {
    fn infos(&self) -> &ProcessInfos {
        &self.infos
    }
    fn next_event(&mut self) -> Option<NoteEvent> {
        None
    }
    fn send_event(&mut self, _: NoteEvent) {}
    fn clear_events(&mut self) {}
    fn events(&self) -> &[NoteEvent] {
        &[]
    }
}

/// Drives the engine over n samples with the given context (silent input).
fn run_ctx(engine: &mut ParticulaEngine, ctx: &mut Box<dyn ProcessContext>, n: usize) {
    for _ in 0..n {
        let mut buf = [0.0_f32];
        engine.process(&mut buf, &[], ctx);
    }
}

/// A beat-synced engine at 120 BPM; interval 0.25 beats (= 16th notes).
fn beat_engine(seed: u64) -> ParticulaEngine {
    let mut e = ParticulaEngine::<1>::new(2048, SR, seed);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0;
    e.spawn_sync = true;
    e.spawn_interval_beats = 0.25;
    e.fallback_bpm = 120.0;
    e.max_particles = 64.0;
    e
}

#[test]
fn beat_sync_spawns_on_the_beat_grid() {
    let mut e = beat_engine(1);
    let mut ctx: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(Some(120.0), true, true));
    run_ctx(&mut e, &mut ctx, SR); // 1 s at 120 BPM = 2 beats

    // next_spawn_beat starts at 0.25 beats (125 ms); at 48 kHz that means
    // spawns at samples 6000, 12000, ..., 42000 -> 7 spawns in 1 s.
    assert!(
        (6..=8).contains(&e.spawned()),
        "beat grid spawn count implausible: {}",
        e.spawned()
    );
}

#[test]
fn beat_sync_falls_back_to_fallback_bpm_without_trusted_tempo() {
    let mut e = beat_engine(2);
    // tempo unknown AND not trustable -> engine falls back to 120 BPM,
    // same beat density as the trusted test.
    let mut ctx: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(None, false, false));
    run_ctx(&mut e, &mut ctx, SR);
    assert!(
        (6..=8).contains(&e.spawned()),
        "fallback spawn count implausible: {}",
        e.spawned()
    );
}

#[test]
fn free_run_mode_unchanged_when_sync_off() {
    let mut e = ParticulaEngine::<1>::new(2048, SR, 3);
    e.dry = 0.0;
    e.wet = 1.0;
    e.texture_blend = 0.0;
    e.spawn_sync = false;
    e.spawn_interval_ms = 40.0;
    let mut ctx: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(Some(120.0), true, true));
    run_ctx(&mut e, &mut ctx, SR);
    // 40 ms interval -> ~25 spawns/s (first one at sample 1).
    assert!(
        (23..=26).contains(&e.spawned()),
        "free-run spawn count implausible: {}",
        e.spawned()
    );
}

#[test]
fn transport_restart_aligns_beat_phase() {
    let mut e = beat_engine(4);

    // Segment 1: playing. 8000 samples -> phase = 0..0.333 beat; the first
    // spawn lands at beat 0.25 (sample 6000), so exactly one spawn.
    let mut ctx: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(Some(120.0), true, true));
    run_ctx(&mut e, &mut ctx, 8_000);
    let after_playing = e.spawned();
    assert!(after_playing == 1, "first beat spawn expected, got {after_playing}");

    // Segment 2: transport stops (engine keeps a free-running fallback
    // metronome while paused, spawns may still fire - just snapshot).
    let mut ctx_stopped: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(Some(120.0), false, true));
    run_ctx(&mut e, &mut ctx_stopped, 2_000);
    let at_stop = e.spawned();

    // Segment 3: transport restarts (playing false->true). The engine resets
    // the beat phase to 0 and the next spawn waits a full interval (6000
    // samples); 2000 samples later nothing may have spawned yet.
    let mut ctx_resumed: Box<dyn ProcessContext> =
        Box::new(TestCtx::new(Some(120.0), true, true));
    run_ctx(&mut e, &mut ctx_resumed, 2_000);
    assert!(
        e.spawned() == at_stop,
        "transport restart must not spawn before the next beat grid point (was {at_stop}, now {})",
        e.spawned()
    );
}