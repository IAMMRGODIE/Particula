//! The particle cloud engine: shared history + particle pool + spawn rule.

use std::{
    f32::consts::PI,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
};

use crossbeam_channel::Sender;
use i_am_dsp::{Effect, ProcessContext, tools::{bpm_syncer::BpmSyncer, ring_buffer::RingBuffer}};
use i_am_dsp_derive::Parameters;

use crate::{
    history::recent_peak_position,
    particle::Particle,
    position_mod::{LfoWave, PositionMod},
    rng::SplitMix64,
    spawner::{SpawnEvent, Spawner},
    texture::Texture,
};

/// Fixed pool capacity of the slot map (Architecture.md sec.10: 64~256).
pub const DEFAULT_POOL_CAPACITY: usize = 192;

/// Equal-power pan gains across the channel count.
///
/// CHANNELS = 1: mono. CHANNELS = 2: constant-power L/R from pan in [-1, 1]
/// (pan -1 = hard left). Larger counts: equal distribution (pan ignored).
fn pan_gains<const CHANNELS: usize>(pan: f32) -> [f32; CHANNELS] {
    let mut g = [0.0_f32; CHANNELS];
    match CHANNELS {
        0 => {}
        1 => g[0] = 1.0,
        2 => {
            let t = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
            g[0] = (t * PI * 0.5).cos();
            g[1] = (t * PI * 0.5).sin();
        }
        n => {
            let inv = 1.0 / n as f32;
            g.fill(inv);
        }
    }
    g
}

/// The core engine: shared mono history + particle pool + spawn rule.
///
/// Generic over CHANNELS: dry input is mixed to mono and pushed into the
/// shared history; every particle voice is pan-distributed across channels
/// with equal-power gains (stereo) or equal gains (mono / >2 channels).
///
/// Data flow per sample (see Architecture.md sec.2):
/// 1. dry input pushed at the write head;
/// 2. spawner rules decide whether a particle is born (arithmetic position
///    sequence + jitter + exponential strength decay);
/// 3. every live particle reads the history (optionally blended with the
///    WSOLA texture layer, v2) at its smoothed position with cubic
///    interpolation, applies playback rate, IIR-Hilbert frequency shift, the
///    envelope, and writes feedback back into the history (v1, serial
///    semantics);
/// 4. output = dry * input + wet.
///
/// No BPM sync yet.
#[derive(Parameters)]
pub struct ParticulaEngine<const CHANNELS: usize = 1> {
    // --- live-tweakable parameters (host-visible) ---
    #[range(min = 0.0, max = 1.0)]
    pub dry: f32,
    /// Wet mix / output compensation gain: allowed beyond 1.0 (up to +12 dB)
    /// to make up for a quiet particle bed.
    #[range(min = 0.0, max = 4.0)]
    pub wet: f32,
    /// Master bypass: when off the effect passes the input through untouched
    /// (the history is left running silent and no particles are processed).
    pub enabled: bool,

    #[range(min = 1.0, max = 5000.0)]
    pub spawn_interval_ms: f32,
    #[range(min = 1.0, max = 192.0)]
    pub max_particles: f32,

    // Spawn timing sync (v2 BPM sync, Architecture.md sec.7).
    pub spawn_sync: bool,
    #[range(min = 0.03125, max = 16.0)]
    pub spawn_interval_beats: f32,
    #[range(min = 20.0, max = 300.0)]
    pub fallback_bpm: f32,

    // Spawn rule: arithmetic position sequence + jitter + exp strength decay.
    #[range(min = 0.0, max = 1.0)]
    pub base_position: f32,
    #[range(min = -1.0, max = 1.0)]
    pub position_step: f32,
    #[range(min = 0.0, max = 1.0)]
    pub position_jitter: f32,
    #[range(min = 0.001, max = 1.0)]
    #[logarithmic]
    pub gain_decay_ratio: f32,
    /// Floor for the generational strength decay: the cloud never gets
    /// quieter than `initial_gain * min_gain_ratio`, so spawning keeps
    /// producing audible particles forever. Set 0 to let it decay into
    /// silence (the pre-floor behaviour). Default 0.05 = -26 dB.
    #[range(min = 0.0, max = 1.0)]
    pub min_gain_ratio: f32,
    #[range(min = 0.0, max = 1.0)]
    pub initial_gain: f32,

    // Particle shape.
    #[range(min = 0.0, max = 100.0)]
    pub attack_ms: f32,
    #[range(min = 1.0, max = 10000.0)]
    pub lifetime_ms_min: f32,
    #[range(min = 1.0, max = 10000.0)]
    pub lifetime_ms_max: f32,
    #[range(min = 0.05, max = 8.0)]
    #[logarithmic]
    pub pitch_min: f32,
    #[range(min = 0.05, max = 8.0)]
    #[logarithmic]
    pub pitch_max: f32,
    #[range(min = -5000.0, max = 5000.0)]
    pub freq_shift_min: f32,
    #[range(min = -5000.0, max = 5000.0)]
    pub freq_shift_max: f32,

    // Reverse playback: a fraction of the spawned particles read the history
    // backwards (negative playback rate = drift towards older samples).
    #[range(min = 0.0, max = 1.0)]
    pub reverse_chance: f32,

    // Position smoothing.
    #[range(min = 0.1, max = 1000.0)]
    pub position_smooth_ms: f32,

    // Position modulation source (0 = fixed, 1 = LFO, 2 = random walk,
    // 3 = peak follow).
    #[range(min = 0, max = 3)]
    pub position_mode: i32,
    /// LFO waveform: 0 Sine, 1 Triangle, 2 Saw, 3 Square.
    #[range(min = 0, max = 3)]
    pub lfo_wave: i32,
    /// LFO rate in beats (used when the BPM-grid sync is on).
    #[range(min = 0.03125, max = 8.0)]
    pub lfo_rate_beats: f32,
    #[range(min = 0.01, max = 50.0)]
    pub lfo_rate_hz: f32,
    #[range(min = 0.0, max = 0.5)]
    pub lfo_depth: f32,
    #[range(min = 0.0, max = 0.25)]
    pub random_walk_step: f32,
    #[range(min = 1.0, max = 2000.0)]
    pub random_walk_interval_ms: f32,
    #[range(min = 0.03125, max = 16.0)]
    pub random_walk_interval_beats: f32,
    #[range(min = 1.0, max = 2000.0)]
    pub peak_window_ms: f32,
    #[range(min = 1.0, max = 1000.0)]
    pub peak_update_ms: f32,
    #[range(min = 0.03125, max = 16.0)]
    pub peak_update_beats: f32,
    #[range(min = 0.0, max = 1.0)]
    pub peak_threshold: f32,

    // Feedback (v1).
    #[range(min = 0.0, max = 0.99)]
    pub feedback_gain: f32,
    /// Feedback injection distance from the freshest sample, in milliseconds
    /// (used when the BPM-grid sync is off).
    #[range(min = 0.0, max = 2000.0)]
    pub feedback_delay_ms: f32,
    /// Same distance in beats, used when `spawn_sync` (BPM grid) is on so the
    /// feedback rides the transport tempo.
    #[range(min = 0.03125, max = 16.0)]
    pub feedback_delay_beats: f32,
    #[range(min = 0.0, max = 20000.0)]
    pub feedback_damping_hz: f32,

    // History length: the delay line is runtime-resizable; the capacity is
    // quantized to a power of two so the particle hot-loop mask stays valid.
    #[range(min = 16.0, max = 5000.0)]
    pub history_len_ms: f32,
    #[range(min = 0.03125, max = 16.0)]
    pub history_len_beats: f32,

    // WSOLA texture layer (v2).
    #[range(min = 0.0, max = 1.0)]
    pub texture_blend: f32,
    #[range(min = 20.0, max = 2000.0)]
    pub texture_window_ms: f32,
    #[range(min = 5.0, max = 1000.0)]
    pub texture_refresh_ms: f32,
    #[range(min = 0.25, max = 4.0)]
    #[logarithmic]
    pub texture_stretch: f32,
    #[range(min = 1.0, max = 200.0)]
    pub texture_crossfade_ms: f32,

    // Particle pan distribution (v2 stereo, per spawn).
    #[range(min = -1.0, max = 1.0)]
    pub pan_min: f32,
    #[range(min = -1.0, max = 1.0)]
    pub pan_max: f32,

    // --- internal state (never host parameters) ---
    #[skip]
    history: RingBuffer<f32>,
    #[skip]
    bpm: BpmSyncer,
    #[skip]
    next_spawn_beat: f32,
    #[skip]
    was_playing: bool,
    /// Whether the beat phase currently comes from the host (current_beat_number)
    /// or the internal counter; used to detect mid-song playhead jumps.
    #[skip]
    use_host_phase: bool,
    #[skip]
    prev_beat: f32,
    #[skip]
    texture: Texture,
    #[skip]
    slots: Vec<Option<Particle>>,
    #[skip]
    free: Vec<usize>,
    /// PANIC latch: the GUI flips it, the audio thread consumes it once and
    /// wipes the delay line + particle pool (see `clear_all`).
    #[skip]
    panic_flag: Arc<AtomicBool>,
    /// True while the host provides a reliable BPM or beat position, so the
    /// UI can hide the fallback BPM control.
    #[skip]
    host_tempo_known: Arc<AtomicBool>,
    #[skip]
    spawner: Spawner,
    #[skip]
    rng: SplitMix64,
    #[skip]
    // GUI notification: every spawned particle is posted here (optional).
    spawn_notifier: Option<Sender<SpawnEvent>>,
    #[skip]
    sample_count: usize,
    #[skip]
    sample_rate: usize,
    #[skip]
    spawn_count: usize,
    #[skip]
    peak_position_t: f32,
    #[skip]
    next_peak_update: usize,
}

impl<const CHANNELS: usize> Default for ParticulaEngine<CHANNELS> {
    fn default() -> Self {
        Self::new(1 << 15, 48_000, 0x5EED_FA11)
    }
}

impl<const CHANNELS: usize> ParticulaEngine<CHANNELS> {
    /// Creates a new engine.
    ///
    /// history_capacity bounds the maximum readable delay in samples;
    /// changing it during a run invalidates the buffer content (see
    /// Architecture.md sec.3.2), so pick it up front.
    pub fn new(history_capacity: usize, sample_rate: usize, seed: u64) -> Self {
        Self {
            dry: 1.0,
            wet: 1.0,
            enabled: true,
            spawn_interval_ms: 30.0,
            max_particles: 64.0,
            spawn_sync: true,
            spawn_interval_beats: 0.25,
            fallback_bpm: 120.0,
            // Near-live audio: the cloud is audible from the first second
            // instead of waiting for half the history buffer to fill.
            base_position: 0.9,
            position_step: 0.0,
            position_jitter: 0.02,
            gain_decay_ratio: 0.9,
            min_gain_ratio: 0.5,
            initial_gain: 0.8,
            attack_ms: 10.0,
            lifetime_ms_min: 100.0,
            lifetime_ms_max: 1200.0,
            pitch_min: 0.5,
            pitch_max: 1.5,
            freq_shift_min: -120.0,
            freq_shift_max: 120.0,
            reverse_chance: 0.0,
            position_smooth_ms: 20.0,
            position_mode: 1,
            lfo_wave: 0,
            lfo_rate_beats: 1.0,
            lfo_rate_hz: 0.15,
            lfo_depth: 0.15,
            random_walk_step: 0.02,
            random_walk_interval_ms: 200.0,
            random_walk_interval_beats: 1.0,
            peak_window_ms: 150.0,
            peak_update_ms: 30.0,
            peak_update_beats: 1.0,
            peak_threshold: 0.01,
            feedback_gain: 0.0,
            feedback_delay_ms: 40.0,
            feedback_delay_beats: 1.0,
            feedback_damping_hz: 3000.0,
            history_len_ms: history_capacity.max(1) as f32 * 1000.0
                / sample_rate.max(1) as f32,
            history_len_beats: 2.0,
            texture_blend: 0.35,
            texture_window_ms: 85.0,
            texture_refresh_ms: 43.0,
            texture_stretch: 1.0,
            texture_crossfade_ms: 12.0,
            pan_min: -0.8,
            pan_max: 0.8,
            history: RingBuffer::new(history_capacity.max(1)),
            bpm: BpmSyncer::new(sample_rate.max(1)),
            next_spawn_beat: 0.25,
            was_playing: false,
            use_host_phase: false,
            prev_beat: 0.0,
            texture: Texture::new(
                (0.085 * sample_rate as f32) as usize,
                sample_rate.max(1),
            ),
            slots: Vec::with_capacity(DEFAULT_POOL_CAPACITY),
            free: Vec::with_capacity(DEFAULT_POOL_CAPACITY),
            panic_flag: Arc::new(AtomicBool::new(false)),
            host_tempo_known: Arc::new(AtomicBool::new(false)),
            spawner: Spawner::new(),
            rng: SplitMix64::new(seed),
            spawn_notifier: None,
            sample_count: 0,
            sample_rate,
            spawn_count: 0,
            peak_position_t: 0.5,
            next_peak_update: 0,
        }
    }

    /// Reseeds the RNG (reproducible patches).
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = SplitMix64::new(seed);
    }

    /// The PANIC latch: set it from any thread; the audio thread consumes it
    /// once on the next process() call and clears history + particles.
    pub fn panic_flag(&self) -> Arc<AtomicBool> {
        self.panic_flag.clone()
    }

    /// Host-BPM/beat availability latch, refreshed each process() call so the
    /// GUI can conditionally hide the fallback BPM control.
    pub fn host_tempo_known_flag(&self) -> Arc<AtomicBool> {
        self.host_tempo_known.clone()
    }

    /// Resizes the history delay line, preserving the freshest tail. The
    /// capacity is quantized to a power of two (particle reads use a mask).
    fn resize_history(&mut self, cap: usize) {
        let old = self.history.capacity();
        if cap == 0 || old == cap {
            return;
        }
        let keep = old.min(cap);
        let mut freshest_first = Vec::with_capacity(keep);
        for i in 0..keep {
            let slot = (self.history.current_pos() + old - 1 - i) % old;
            freshest_first.push(self.history[slot]);
        }
        let mut nh = RingBuffer::new(cap);
        for s in freshest_first.iter().rev() {
            nh.push(*s);
        }
        self.history = nh;
    }

    /// Wipes the delay line and kills every particle (PANIC button).
    fn clear_all(&mut self) {
        self.history.underlying_buffer_mut().fill(0.0);
        self.texture.clear();
        for s in self.slots.iter_mut() {
            *s = None;
        }
        self.free = (0..self.slots.len()).collect();
    }

    /// Attaches an optional spawn-event notifier. The engine posts one
    /// `SpawnEvent` per born particle over this channel (unbounded push, no
    /// risk of blocking the audio thread).
    pub fn set_spawn_notifier(&mut self, tx: Sender<SpawnEvent>) {
        self.spawn_notifier = Some(tx);
    }

    /// Current history capacity in samples (tests / debug).
    pub fn history_capacity_for_test(&self) -> usize {
        self.history.capacity()
    }

    /// Clears the pool and the spawn sequence (keeps history).
    pub fn clear_particles(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.spawner.reset(0);
    }

    /// Number of live particles right now.
    pub fn live_count(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    /// Total particles born since construction / last clear.
    pub fn spawned(&self) -> usize {
        self.spawn_count
    }

    /// Samples processed since construction / last clear.
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// The sample rate the engine is currently running at.
    pub fn sample_rate(&self) -> usize {
        self.sample_rate
    }

    /// Read positions (t-space) of all live particles, in slot order.
    /// Useful for meters and for verifying reverse-playback behaviour in
    /// tests.
    pub fn particle_positions(&self) -> Vec<f32> {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref().map(|p| p.position))
            .collect()
    }

    /// The spawn rule's arithmetic position for generation n
    /// (t-space, wrapped). Pure: jitter = 0 gives the exact sequence.
    pub fn spawn_rule_position(&self, n: usize) -> f32 {
        (self.base_position + n as f32 * self.position_step).rem_euclid(1.0)
    }

    /// The spawn rule's strength (linear gain) for generation n:
    /// `initial_gain * max(decay_ratio^n, min_gain_ratio)`. The exponential
    /// decay bottoms out at the min_gain_ratio floor so the cloud cannot
    /// silently exhaust itself a few seconds in.
    pub fn spawn_rule_gain(&self, n: usize) -> f32 {
        (self.initial_gain * self.gain_decay_ratio.powi(n as i32))
            .max(self.initial_gain * self.min_gain_ratio)
    }

    /// Builds one particle from the current parameters (spawn rule + shape).
    fn make_particle(&mut self, sample_rate: usize, tempo: f32) -> Particle {
        let n = self.spawner.sequence_index();
        let position = (self.spawn_rule_position(n)
            + self.position_jitter * self.rng.sym())
        .rem_euclid(1.0);
        let gain = self.spawn_rule_gain(n);
        let magnitude = self.rng.range(self.pitch_min, self.pitch_max);
        // Reverse playback: negative rate walks the read head towards older
        // samples (Architecture.md sec.5.3). Only consumes RNG when enabled,
        // so the default (chance = 0) keeps the exact previous RNG stream.
        let rate = if self.reverse_chance > 0.0
            && self.rng.next_f32() < self.reverse_chance
        {
            -magnitude
        } else {
            magnitude
        };
        let shift = self.rng.range(self.freq_shift_min, self.freq_shift_max);
        let pan = self.rng.range(self.pan_min, self.pan_max);
        let lmin = (self.lifetime_ms_min * sample_rate as f32 / 1000.0) as usize;
        let lmax = (self.lifetime_ms_max * sample_rate as f32 / 1000.0) as usize;
        let lifetime = self.rng.range_usize(lmin, lmax);
        let attack = ((self.attack_ms * sample_rate as f32 / 1000.0).max(1.0)) as usize;
        let mode = match self.position_mode {
            0 => PositionMod::fixed(),
            1 => {
                let rate_hz = if self.spawn_sync {
                    (self.lfo_rate_beats.max(0.015625) * tempo.max(1.0) / 60.0)
                        .max(0.0001)
                } else {
                    self.lfo_rate_hz
                };
                PositionMod::lfo(
                    LfoWave::from(self.lfo_wave),
                    rate_hz,
                    self.lfo_depth,
                    &mut self.rng,
                )
            }
            2 => PositionMod::random_walk(
                self.random_walk_step,
                self.lfo_depth,
                if self.spawn_sync {
                    (self.random_walk_interval_beats.max(0.03125) * tempo.max(1.0)
                        / 60.0
                        * sample_rate as f32) as usize
                } else {
                    (self.random_walk_interval_ms * sample_rate as f32 / 1000.0) as usize
                }
                .max(1),
                &mut self.rng,
            ),
            _ => PositionMod::peak_follow(),
        };
        Particle::new(
            sample_rate,
            position,
            mode,
            rate,
            shift,
            gain,
            self.feedback_gain,
            pan,
            attack,
            lifetime,
            self.position_smooth_ms,
        )
    }

    fn set_sample_rate(&mut self, sample_rate: usize) {
        // v0 limitation: existing particles keep their construction sample
        // rate (their Hilbert/Biquad coefficients); a sample-rate change only
        // affects newly spawned particles. TODO: rebuild live particle state.
        self.sample_rate = sample_rate;
        self.bpm = BpmSyncer::new(sample_rate.max(1));
        self.next_spawn_beat = self.spawn_interval_beats.max(0.03125);
    }
}

impl<const CHANNELS: usize> Effect<CHANNELS> for ParticulaEngine<CHANNELS> {
    fn delay(&self) -> usize {
        // No FIR / WSOLA yet: zero latency.
        0
    }

    fn process(
        &mut self,
        samples: &mut [f32; CHANNELS],
        _other: &[&[f32; CHANNELS]],
        ctx: &mut Box<dyn ProcessContext>,
    ) {
        let infos = ctx.infos();
        let new_sr = if infos.trustable && infos.sample_rate != 0 {
            infos.sample_rate
        } else {
            48_000
        };
        if new_sr != self.sample_rate {
            self.set_sample_rate(new_sr);
        }
        let sample_rate = self.sample_rate;

        // 0a. PANIC: consume the latch once per call and wipe history/particles.
        if self.panic_flag.swap(false, Ordering::Relaxed) {
            self.clear_all();
        }
        // Refreshes host tempo/beat availability for the GUI.
        let host_known = infos.trustable
            && (infos.tempo.is_some_and(|t| t > 0.0) || infos.current_beat_number.is_some());
        self.host_tempo_known.store(host_known, Ordering::Relaxed);

        // 0b. History length: beats (BPM grid on) or ms; rebuilds the delay
        //     line when the target capacity changes, keeping the freshest tail.
        let tempo = infos
            .tempo
            .filter(|t| *t > 0.0 && infos.trustable)
            .unwrap_or(self.fallback_bpm);
        let hist_target = if self.spawn_sync {
            (self.history_len_beats.max(0.03125) * 60.0 / tempo.max(1.0)
                * sample_rate as f32) as usize
        } else {
            (self.history_len_ms * sample_rate as f32 / 1000.0) as usize
        };
        let hist_target = hist_target
            .max(256)
            .next_power_of_two()
            .min(1 << 18);
        if hist_target != self.history.capacity() {
            self.resize_history(hist_target);
        }

        // 0c. Master bypass: straight passthrough, leave the buffer untouched.
        if !self.enabled {
            return;
        }

        // 1. dry input: mono mix into the shared history (dry path keeps
        //    each input channel untouched).
        let dry_in = *samples;
        let input = dry_in.iter().sum::<f32>() / CHANNELS as f32;
        self.history.push(input);
        self.sample_count += 1;

        // 2b. BPM sync accumulator + transport restart detection
        //     (Architecture.md sec.7).
        let tempo = if infos.trustable {
            infos.tempo.filter(|t| *t > 0.0).unwrap_or(self.fallback_bpm)
        } else {
            self.fallback_bpm
        };
        self.bpm.next_k(tempo, 1);
        if infos.playing && !self.was_playing {
            // Transport restart: re-align the beat phase to 0.
            self.bpm.reset();
            self.next_spawn_beat = self.spawn_interval_beats.max(0.03125);
        }
        self.was_playing = infos.playing;

        // 3. spawn scheduling: beat-quantized when spawn_sync, otherwise a
        //    free-running millisecond interval (Architecture.md sec.6/7).
        //
        // Beat position (two-layer detection):
        //  layer 1 — transport says playing AND the host reports a beat number:
        //    use it; otherwise (paused, or no host beat) fall back to the
        //    internal BPM counter. A paused transport leaves current_beat_number
        //    frozen, but the user may still be writing/previewing, so the cloud
        //    keeps running on the internal grid there.
        //  layer 2 — while on the host path, a fast forward/backward jump of
        //    the playhead (mid-song seek) realigns `next_spawn_beat` to the
        //    next grid point after the playhead; no burst, no stale targets.
        let interval = self.spawn_interval_beats.max(0.03125);
        let use_host = infos.playing
            && infos.trustable
            && infos.current_beat_number.is_some();
        let beat_now = if use_host {
            infos.current_beat_number.unwrap_or(self.bpm.read())
        } else {
            self.bpm.read()
        };
        let switched = use_host != self.use_host_phase
            || (self.prev_beat - beat_now).abs() > interval * 2.0 + 0.001;
        self.use_host_phase = use_host;
        self.prev_beat = beat_now;
        let spawn_due = if self.spawn_sync {
            if switched {
                // Realign to the grid point after the playhead (may be in the
                // future after a forward seek — nothing fires until it lands).
                self.next_spawn_beat =
                    (beat_now / interval).floor() * interval + interval;
            }
            let due = beat_now >= self.next_spawn_beat;
            if due {
                self.next_spawn_beat += interval;
            }
            due
        } else {
            let interval = ((self.spawn_interval_ms * sample_rate as f32 / 1000.0).max(1.0))
                as usize;
            self.spawner.poll(self.sample_count, interval)
        };
        if spawn_due {
            self.spawner.bump_sequence();
            let lives = self.live_count();
            if lives < self.max_particles.max(1.0) as usize && lives < DEFAULT_POOL_CAPACITY {
                let idx = self
                    .free
                    .pop()
                    .unwrap_or_else(|| {
                        self.slots.push(None);
                        self.slots.len() - 1
                    });
                self.slots[idx] = Some(self.make_particle(sample_rate, tempo));
                self.spawn_count += 1;
                if let (Some(tx), Some(p)) = (&self.spawn_notifier, &self.slots[idx]) {
                    let _ = tx.send(SpawnEvent {
                        lifetime_samples: p.lifetime(),
                        live: self.live_count(),
                    });
                }
            }
        }

        // 3. shared peak-follow target (periodic update of the loudest
        //    sample in the recent history window).
        let update = if self.spawn_sync {
            ((self.peak_update_beats.max(0.03125) * 60.0 / tempo.max(1.0))
                * sample_rate as f32) as usize
        } else {
            (self.peak_update_ms * sample_rate as f32 / 1000.0) as usize
        }
        .max(1);
        if self.sample_count >= self.next_peak_update {
            let window = ((self.peak_window_ms * sample_rate as f32 / 1000.0) as usize).max(1);
            self.peak_position_t = recent_peak_position(&self.history, window, self.peak_threshold);
            self.next_peak_update = self.sample_count + update;
        }

        // 3b. WSOLA texture layer: slide the tap, refresh on schedule.
        let window_samples = ((self.texture_window_ms * sample_rate as f32 / 1000.0) as usize).max(64);
        if window_samples != self.texture.window_capacity() {
            self.texture.resize(window_samples);
        }
        let refresh = ((self.texture_refresh_ms * sample_rate as f32 / 1000.0) as usize).max(1);
        let fade = ((self.texture_crossfade_ms * sample_rate as f32 / 1000.0) as usize).max(1);
        self.texture.process(
            &self.history,
            self.texture_stretch,
            refresh,
            fade,
            self.sample_count,
        );

        // 4. particles: read, pitch, shift, envelope, sum; serial feedback
        //    writes into the history (Architecture.md sec.3.1: later
        //    particles see earlier ones' feedback this frame).
        let mut wet = [0.0_f32; CHANNELS];
        let dt = 1.0 / sample_rate as f32;
        // Feedback delay: beats (BPM grid on) or milliseconds (off).
        let feedback_delay = if self.spawn_sync {
            ((self.feedback_delay_beats.max(0.03125) * 60.0 / tempo.max(1.0))
                * sample_rate as f32) as usize
        } else {
            (self.feedback_delay_ms * sample_rate as f32 / 1000.0) as usize
        }
        .min(self.history.capacity());
        let fb_lp_a = if self.feedback_damping_hz <= 0.0 {
            1.0
        } else {
            1.0 - (-2.0 * PI * self.feedback_damping_hz / sample_rate as f32).exp()
        };
        let peak_t = self.peak_position_t;
        let sample_count = self.sample_count;
        let (rng, history) = (&mut self.rng, &mut self.history);
        let mut i = 0;
        while i < self.slots.len() {
            let Some(p) = &mut self.slots[i] else {
                i += 1;
                continue;
            };
            match p.process(
                history,
                &self.texture,
                self.texture_blend,
                dt,
                sample_count,
                peak_t,
                feedback_delay,
                fb_lp_a,
                rng,
                ctx,
            ) {
                Some(s) => {
                    let gains = pan_gains::<CHANNELS>(p.pan);
                    for (w, g) in wet.iter_mut().zip(gains.iter()) {
                        *w += s * g;
                    }
                },
                None => {
                    self.slots[i] = None;
                    self.free.push(i);
                },
            }
            i += 1;
        }

        // 5. dry (per channel) + wet (pan-distributed particle voices).
        for c in 0..CHANNELS {
            samples[c] = dry_in[c] * self.dry + wet[c];
        }
    }
}