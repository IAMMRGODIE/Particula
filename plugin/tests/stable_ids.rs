//! The host-facing parameter ids of a released build are frozen by its users' projects.
//!
//! A host keys the automation and modulation it stores on the id the plugin hands out, and
//! CLAP forbids changing the id of an existing parameter: a moved id silently rewires every
//! project that was saved against the old one. `ParticulaProcessor` therefore builds its
//! parameter map with `StableIdScheme::Position`, which reproduces the positional ids that
//! shipped before ids were derived from identifiers.
//!
//! Under that scheme the position *is* the id, so the declaration order below is the actual
//! contract. Appending a parameter at the end is safe (and requires adding it here); inserting
//! or reordering one anywhere else moves every id after it and this test fails on purpose.
//! New parameters can avoid the whole question by pinning `#[id(stable = N)]`.

use i_am_dsp::prelude::Parameters;
use i_am_plugin::Plugin;
use particula_plugin::ParticulaProcessor;

/// Every host-visible parameter, in the order the plugin declares them.
const FROZEN_PARAMETER_ORDER: [&str; 48] = [
    "dry", "wet", "enabled", "spawn_interval_ms",
    "max_particles", "spawn_sync", "spawn_interval_beats", "fallback_bpm",
    "base_position", "position_step", "position_jitter", "gain_decay_ratio",
    "min_gain_ratio", "initial_gain", "attack_ms", "lifetime_ms_min",
    "lifetime_ms_max", "pitch_min", "pitch_max", "freq_shift_min",
    "freq_shift_max", "reverse_chance", "position_smooth_ms", "position_mode",
    "lfo_wave", "lfo_rate_beats", "lfo_rate_hz", "lfo_depth",
    "random_walk_step", "random_walk_interval_ms", "random_walk_interval_beats", "peak_window_ms",
    "peak_update_ms", "peak_update_beats", "peak_threshold", "feedback_gain",
    "feedback_delay_ms", "feedback_delay_beats", "feedback_damping_hz", "history_len_ms",
    "history_len_beats", "texture_blend", "texture_window_ms", "texture_refresh_ms",
    "texture_stretch", "texture_crossfade_ms", "pan_min", "pan_max",
];

#[test]
fn released_parameter_ids_are_frozen() {
    let processor = ParticulaProcessor::new(48_000);
    let param_map = processor.param_map();
    let parameters = processor.get_parameters();

    assert_eq!(
        parameters.len(),
        FROZEN_PARAMETER_ORDER.len(),
        "the parameter count changed: appending is fine, but it has to be recorded here"
    );

    for (index, (parameter, expected)) in parameters.iter().zip(FROZEN_PARAMETER_ORDER).enumerate() {
        assert_eq!(
            parameter.identifier, expected,
            "parameter {index} is out of order, which moves its id and the ids after it"
        );
        assert_eq!(
            param_map.query_stable_id(index),
            Some(index as u32),
            "parameter {index} (`{expected}`) no longer has its released id"
        );
        assert_eq!(param_map.query_param_index_by_stable_id(index as u32), Some(index));
    }
}