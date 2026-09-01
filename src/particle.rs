//! One granular voice: read position, pitch drift, envelope, freq shifter,
//! serial feedback write.

use i_am_dsp::{
    Effect, ProcessContext,
    effects::freq_shifter::IIRFreqShifter,
    generators::wavetable::WaveTable,
    tools::{ring_buffer::RingBuffer, smoother::DoubleTimeConstant},
};

use crate::{
    history::add_at,
    position_mod::PositionMod,
    rng::SplitMix64,
    texture::Texture,
};

/// Cheap smooth soft-clip for the feedback write (part of the stability
/// trio: keeps any single write bounded).
#[inline]
fn soft_clip(x: f32) -> f32 {
    x / (1.0 + x.abs())
}

/// Filter order for the per-particle IIR Hilbert transform (constant, because
/// the particle pool requires a concrete type; 4 is a good cheap balance).
pub const FREQ_SHIFTER_ORDER: usize = 4;

/// A single granular voice.
///
/// Read model (all in WaveTable t-space [0, 1), see Architecture.md sec.4):
/// every sample the position advances by playback_rate / capacity (pitch
/// drift) and is pulled toward the modulated onset through a smoothed
/// follower (position smoothing, prevents clicks). The envelope is a short
/// linear attack followed by an exponential decay down to -60 dB at the end
/// of the lifetime.
///
/// Feedback (v1, Architecture.md sec.3.1/8): after the voice sample is
/// computed, feedback_gain * voice is soft-clipped, damped by a one-pole
/// lowpass and written into the shared history at the engine-chosen
/// injection point (delay from the freshest sample). Serial semantics:
/// particles later in the same sample's iteration can read this value.
pub struct Particle {
    /// Current read head position in t-space.
    pub position: f32,
    /// Fixed onset picked at birth (spawn rule).
    onset: f32,
    /// Accumulated pitch drift in t-space.
    drift: f32,
    /// Follower toward the modulated onset.
    smoother: DoubleTimeConstant<1>,
    /// Position modulation source.
    position_mod: PositionMod,
    /// Granular playback rate (1.0 = original speed). A *negative* rate
    /// plays backwards: the drift walks the read head towards older history
    /// samples (reverse playback, `ParticulaEngine::reverse_chance`).
    pub playback_rate: f32,
    /// Per-particle frequency shifter (stateful: Hilbert + Biquad + phase).
    freq_shifter: IIRFreqShifter<FREQ_SHIFTER_ORDER, 1>,
    /// Current linear gain (includes attack ramp + decay).
    gain: f32,
    initial_gain: f32,
    /// Feedback amount back into the shared history (per-particle snapshot;
    /// delay distance and damping coefficient come from the engine live).
    feedback_gain: f32,
    /// One-pole state used by the feedback damping lowpass.
    feedback_lp_state: f32,
    /// Spawn-time pan in [-1, 1]; the engine distributes the voice with
    /// equal-power gains (stereo).
    pub pan: f32,
    attack_samples: usize,
    attack_elapsed: usize,
    decay_per_sample: f32,
    lifetime: usize,
}

impl Particle {
    /// Creates a new particle.
    ///
    /// All time-like values are in sample/sample-rate units; smooth_ms is the
    /// double-time-constant follower time for position smoothing.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: usize,
        initial_position: f32,
        position_mod: PositionMod,
        playback_rate: f32,
        freq_shift: f32,
        initial_gain: f32,
        feedback_gain: f32,
        pan: f32,
        attack_samples: usize,
        lifetime_samples: usize,
        smooth_ms: f32,
    ) -> Self {
        let lifetime_samples = lifetime_samples.max(1);
        let attack_samples = attack_samples.min(lifetime_samples);
        // Exponential fade to -60 dB at the end of the lifetime.
        let decay_per_sample = 1e-3f32.powf(1.0 / lifetime_samples as f32);
        Self {
            position: initial_position,
            onset: initial_position,
            drift: 0.0,
            smoother: DoubleTimeConstant::new(smooth_ms, smooth_ms, initial_position, sample_rate),
            position_mod,
            playback_rate,
            freq_shifter: IIRFreqShifter::new(sample_rate, freq_shift),
            gain: 0.0,
            initial_gain: initial_gain.max(0.0),
            feedback_gain: feedback_gain.max(0.0),
            feedback_lp_state: 0.0,
            pan: pan.clamp(-1.0, 1.0),
            attack_samples,
            attack_elapsed: 0,
            decay_per_sample,
            lifetime: lifetime_samples,
        }
    }

    /// True while the particle still produces output.
    pub fn is_alive(&self) -> bool {
        self.lifetime > 0 && self.gain >= 1e-5
    }

    /// Current envelope gain (for debugging / meters).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Advance one sample and return the voice output, or None when dead.
    ///
    /// peak_t is the engine's shared peak-follow target (used only when the
    /// particle's modulation source is PositionMod::PeakFollow).
    /// feedback_delay_samples and feedback_lp_a are the live feedback
    /// injection point and damping coefficient from the engine.
    /// texture/texture_blend blend the WSOLA texture layer into the read
    /// (0 = history only, per Architecture.md sec.9).
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        history: &mut RingBuffer<f32>,
        texture: &Texture,
        texture_blend: f32,
        dt: f32,
        sample_count: usize,
        peak_t: f32,
        feedback_delay_samples: usize,
        feedback_lp_a: f32,
        rng: &mut SplitMix64,
        ctx: &mut Box<dyn ProcessContext>,
    ) -> Option<f32> {
        if self.lifetime == 0 {
            return None;
        }
        self.lifetime -= 1;

        // Modulated & smoothed onset target (PeakFollow replaces the onset
        // with the engine's shared latest peak position).
        let offset = self.position_mod.next_offset(dt, sample_count, rng);
        let base = if matches!(self.position_mod, PositionMod::PeakFollow) {
            peak_t
        } else {
            self.onset
        };
        let target = (base + offset).clamp(0.0, 1.0);
        self.smoother.input_value(&[target]);
        let smoothed = self.smoother.get_smoothed_result()[0];

        // Pitch drift on top; read position stays in [0, 1).
        self.drift += self.playback_rate / history.capacity() as f32;
        self.position = (smoothed + self.drift).rem_euclid(1.0);

        let mut s = history.sample(self.position, 0) * (1.0 - texture_blend)
            + texture.sample(self.position) * texture_blend;

        // Envelope: linear attack, then exponential decay.
        if self.attack_elapsed < self.attack_samples {
            self.attack_elapsed += 1;
            self.gain =
                self.initial_gain * (self.attack_elapsed as f32 / self.attack_samples as f32);
        } else {
            self.gain *= self.decay_per_sample;
        }
        if self.gain < 1e-5 {
            return None;
        }
        s *= self.gain;

        // Per-particle frequency shift (mono).
        let mut mono = [s];
        self.freq_shifter.process(&mut mono, &[], ctx);
        s = mono[0];

        // Feedback write back into the shared history (serial semantics: later
        // particles in the same sample can read this value this frame).
        if self.feedback_gain > 0.0 {
            let fb = soft_clip(s * self.feedback_gain);
            self.feedback_lp_state += feedback_lp_a * (fb - self.feedback_lp_state);
            add_at(history, feedback_delay_samples, self.feedback_lp_state);
        }

        Some(s)
    }
}