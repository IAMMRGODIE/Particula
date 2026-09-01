//! Spawn timing state machine and the spawn description.

/// Description of one freshly born particle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    /// Onset read position, in WaveTable t-space `[0, 1)`.
    pub position: f32,
    /// Initial linear gain.
    pub gain: f32,
    /// Granular playback rate (1.0 = original speed).
    pub playback_rate: f32,
    /// Frequency shift in Hz.
    pub freq_shift: f32,
    /// Lifetime in samples.
    pub lifetime_samples: usize,
}

/// Only the *timing and sequence* state lives here; the shape of a spawn
/// (arithmetic position sequence + jitter + exponential strength decay) is the
/// engine's spawn rule (see `ParticulaEngine::spawn_rule_position/_gain`).
#[derive(Clone, Debug, Default)]
pub struct Spawner {
    next_spawn_at: usize,
    sequence_index: usize,
}

impl Spawner {
    /// Creates a new spawner due immediately at sample 0.
    pub const fn new() -> Self {
        Self { next_spawn_at: 0, sequence_index: 0 }
    }

    /// Resets timing; next spawn happens at `at`.
    pub fn reset(&mut self, at: usize) {
        self.next_spawn_at = at;
        self.sequence_index = 0;
    }

    /// Returns true once a spawn is due at `sample_count` and schedules the
    /// next due time. Spawns are dropped (not queued) when nobody polls.
    pub fn poll(&mut self, sample_count: usize, interval_samples: usize) -> bool {
        if sample_count < self.next_spawn_at {
            return false;
        }
        self.next_spawn_at = sample_count + interval_samples.max(1);
        true
    }

    /// Advances the sequence index (call after a spawn is accepted).
    pub fn bump_sequence(&mut self) {
        self.sequence_index = self.sequence_index.wrapping_add(1);
    }

    /// Current generation index `n` (0-based).
    pub fn sequence_index(&self) -> usize {
        self.sequence_index
    }
}
