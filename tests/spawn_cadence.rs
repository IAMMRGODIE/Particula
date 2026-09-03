//! Spawn-cadence verification: are particles actually spawned on a schedule,
//! and does spawning continue after the opening seconds?

use i_am_dsp::{Effect, NoteEvent, ProcessContext, ProcessInfos};
use particula::{ParticulaEngine, Spawner};

const SR: usize = 48_000;

/// Controllable context: a static ProcessInfos snapshot.
struct TestCtx {
    infos: ProcessInfos,
}
impl TestCtx {
    fn new(tempo: Option<f32>, playing: bool) -> Self {
        let mut infos = ProcessInfos::default();
        infos.sample_rate = SR;
        infos.tempo = tempo;
        infos.playing = playing;
        infos.trustable = true;
        Self { infos }
    }
}
impl ProcessContext for TestCtx {
    fn infos(&self) -> &ProcessInfos { &self.infos }
    fn next_event(&mut self) -> Option<NoteEvent> { None }
    fn send_event(&mut self, _: NoteEvent) {}
    fn clear_events(&mut self) {}
    fn events(&self) -> &[NoteEvent] { &[] }
}

/// Test context pinned to a specific host beat position (in beats).
struct BeatCtx {
    infos: ProcessInfos,
}
impl BeatCtx {
    fn new(beat: f32) -> Self {
        let mut infos = ProcessInfos::default();
        infos.sample_rate = SR;
        infos.tempo = Some(120.0);
        infos.playing = true;
        infos.trustable = true;
        infos.current_beat_number = Some(beat);
        Self { infos }
    }
}
impl ProcessContext for BeatCtx {
    fn infos(&self) -> &ProcessInfos { &self.infos }
    fn next_event(&mut self) -> Option<NoteEvent> { None }
    fn send_event(&mut self, _: NoteEvent) {}
    fn clear_events(&mut self) {}
    fn events(&self) -> &[NoteEvent] { &[] }
}

/// Runs the engine, recording the sample index of every spawn.
fn run_track(engine: &mut ParticulaEngine, ctx: &mut Box<dyn ProcessContext>, n: usize) -> Vec<usize> {
    let mut spawn_times = Vec::new();
    let mut prev = engine.spawned();
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let input = 0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
        let mut buf = [input];
        engine.process(&mut buf, &[], ctx);
        if engine.spawned() > prev {
            spawn_times.push(i);
            prev = engine.spawned();
        }
    }
    spawn_times
}

#[test]
fn spawner_poll_fires_on_a_regular_grid() {
    let mut s = Spawner::new();
    let interval = 1440; // 30 ms at 48 kHz
    let mut events = Vec::new();
    for n in 0..72_000usize {
        if s.poll(n, interval) {
            events.push(n);
        }
    }
    assert!(!events.is_empty());
    assert_eq!(events[0], 0, "first poll fires immediately");
    let gaps: Vec<usize> = events.windows(2).map(|w| w[1] - w[0]).collect();
    for &g in &gaps {
        assert_eq!(g, interval, "poll must fire every interval, gap {g}");
    }
}

#[test]
fn free_run_spawns_every_interval_while_pool_is_not_full() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 11);
    e.spawn_sync = false;
    e.spawn_interval_ms = 30.0;
    e.max_particles = 256.0; // never full with these lifetimes
    e.lifetime_ms_min = 500.0;
    e.lifetime_ms_max = 1500.0;
    e.texture_blend = 0.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(TestCtx::new(None, false));
    let times = run_track(&mut e, &mut ctx, 5 * SR);

    assert!(times.len() > 100, "expected ~166 spawns in 5 s, got {}", times.len());
    // Tail must still be spawning (not just the opening).
    let last = *times.last().unwrap();
    assert!(last > 4 * SR, "last spawn too early: sample {last}");
    let tail = times.iter().filter(|&&t| t > 4 * SR).count();
    assert!(tail >= 20, "tail should still spawn ~1 per 30 ms, got {tail} in last second");
    // Cadence: gaps cluster around the 30 ms interval.
    let gaps: Vec<isize> = times.windows(2).map(|w| (w[1] - w[0]) as isize).collect();
    let mean = gaps.iter().sum::<isize>() as f64 / gaps.len() as f64;
    let target = 30.0 * SR as f64 / 1000.0;
    assert!(
        (mean - target).abs() < target * 0.05,
        "mean gap {mean} should be ~{target}"
    );
}

#[test]
fn pooled_engine_replaces_dead_particles_continuously() {
    // Small pool + short lives: the pool saturates, but every death is
    // immediately refilled, so spawning continues for the whole run.
    let mut e = ParticulaEngine::<1>::new(2048, SR, 22);
    e.spawn_sync = false;
    e.max_particles = 8.0;
    e.spawn_interval_ms = 5.0;
    e.lifetime_ms_min = 40.0;
    e.lifetime_ms_max = 40.0;
    e.texture_blend = 0.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(TestCtx::new(None, false));
    let times = run_track(&mut e, &mut ctx, 5 * SR);

    assert!(e.live_count() <= 8, "pool saturated at capacity");
    let last = *times.last().unwrap();
    assert!(last > 4 * SR, "pool must keep refilling, last spawn {last}");
    assert!(times.len() > 500, "5 s of 40 ms lives should refill often: {}", times.len());
}

#[test]
fn beat_sync_mode_spawns_continuously_on_the_grid() {
    let mut e = ParticulaEngine::<1>::new(4096, SR, 33);
    e.spawn_sync = true;
    e.spawn_interval_beats = 0.25;
    e.fallback_bpm = 120.0;
    e.max_particles = 256.0;
    e.texture_blend = 0.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(TestCtx::new(Some(120.0), true));
    let times = run_track(&mut e, &mut ctx, 5 * SR);

    let last = *times.last().unwrap();
    assert!(last > 4 * SR, "beat sync must keep spawning, last {last}");
    let expected = (5.0 * 120.0 / 60.0) / 0.25; // beats in 5 s / interval
    assert!(
        (times.len() as f64 - expected).abs() < expected * 0.2,
        "spawn count should be near {expected}, got {}",
        times.len()
    );
}

#[test]
fn host_beat_number_drives_the_grid() {
    // Static host beat position: 12.4 (bar 3 + 0.4 into the bar at 4/4).
    // The engine must use it (not the internal counter) and fire spawns on
    // the 0.25-beat grid until the target passes 12.4.
    let mut e = ParticulaEngine::<1>::new(4096, SR, 44);
    e.spawn_sync = true;
    e.spawn_interval_beats = 0.25;
    e.max_particles = 256.0;
    e.texture_blend = 0.0;
    let mut ctx: Box<dyn ProcessContext> = Box::new(BeatCtx::new(12.4));
    let times = run_track(&mut e, &mut ctx, 4096);
    assert!(!times.is_empty(), "host beat position should fire spawns");
    // 0.25..12.25 step 0.25 -> about 49 events.
    assert_eq!(times.len(), 49, "grid should march from 0.25 to 12.25");
}

