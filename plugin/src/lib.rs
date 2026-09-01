//! Particula Cloud — CLAP plugin entry + standalone processor.
//!
//! Host parameter automation flows through Paramed: the engine exposes a
//! derived Parameters table, Paramed mirrors it into an atomic ParamMap
//! (host-facing) and copies host changes into the engine each sample. The GUI
//! (ui.rs) reads and writes the same atomic map, so there is no shared mutable
//! state between the GUI and audio threads.
//!
//! Build (offline):
//!   cargo build --release -p particula_plugin
//! then rename target/release/particula_plugin.dll -> ParticulaCloud.clap

mod ui;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use i_am_dsp::{
    Effect, ProcessContext,
    prelude::{ParamMap, Parameter, Parameters, Paramed, SetValue},
};
use i_am_dsp_iced::Processor;
use i_am_plugin::{Descriptor, Plugin, Tag, WindowOptions, export_clap};
use particula::ParticulaEngine;

use ui::{ParticulaMessage, ParticulaView};

/// The processor: the particle cloud engine behind the host parameter
/// automation layer and the HOMOLOGY-styled control surface.
pub struct ParticulaProcessor {
    /// The parameterized engine (Paramed syncs the atomic map into the engine
    /// every sample).
    engine: Paramed<ParticulaEngine<2>>,
    /// GUI-visible counters, updated on the audio thread and read on the GUI
    /// thread. Order: [live, spawned, sample_rate].
    stats: Arc<[AtomicUsize; 3]>,
}

impl ParticulaProcessor {
    /// Creates a new processor. The engine adapts to the sample rate the
    /// host reports at runtime, so the initial value is just a starting point.
    pub fn new(sample_rate: usize) -> Self {
        Self {
            engine: Paramed::new(ParticulaEngine::<2>::new(
                1 << 16, // 1.36 s of history at 48 kHz
                sample_rate.max(1),
                0x5EED_FA11,
            )),
            stats: Arc::new([AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)]),
        }
    }

    fn refresh_stats(&self) {
        let sc = self.engine.value.sample_count();
        if sc & 0b1111111111 == 0 {
            // ~1024 samples: a cheap periodic snapshot for the GUI.
            self.stats[0].store(self.engine.value.live_count(), Ordering::Relaxed);
            self.stats[1].store(self.engine.value.spawned(), Ordering::Relaxed);
            self.stats[2].store(self.engine.value.sample_rate(), Ordering::Relaxed);
        }
    }
}

impl Parameters for ParticulaProcessor {
    fn get_parameters(&self) -> Vec<Parameter> {
        self.engine.get_parameters()
    }

    fn set_parameter(&mut self, identifier: &str, value: SetValue) -> bool {
        self.engine.set_parameter(identifier, value)
    }
}

impl Processor for ParticulaProcessor {
    type Message = ParticulaMessage;
    type SyncedView = ParticulaView;

    fn delay(&self) -> usize {
        self.engine.delay()
    }

    fn on_message(&self, message: Self::Message) {
        match message {
            ParticulaMessage::Param { id, value } => {
                // Straight into the shared atomic map; the audio thread picks
                // it up via Paramed::sync_params on the next sample.
                self.engine.param_map().set(id, value, Ordering::Relaxed);
            }
            ParticulaMessage::Tick => {}
        }
    }

    fn process(
        &mut self,
        samples: &mut [f32; 2],
        other: &[[f32; 2]],
        process_context: &mut Box<dyn ProcessContext>,
    ) {
        // Single Main input port: other (sidechain inputs) stays empty, so no
        // per-sample conversion allocation is needed in the common path.
        if other.is_empty() {
            self.engine.process(samples, &[], process_context);
        } else {
            let refs: Vec<&[f32; 2]> = other.iter().collect();
            self.engine.process(samples, &refs, process_context);
        }
        self.refresh_stats();
    }

    fn synced_view(&self) -> Self::SyncedView {
        // The view owns Arc handles into the shared state and reads/writes
        // them live on every frame; nothing here needs a snapshot.
        ParticulaView {
            param_map: self.engine.param_map(),
            stats: self.stats.clone(),
        }
    }
}

impl Plugin for ParticulaProcessor {
    const DESCRIPTOR: Descriptor = Descriptor::new("dev.particula.cloud", "Particula Cloud")
        .with_tags(&[Tag::AudioEffect, Tag::Granular])
        .with_vendor("particula")
        .with_version(env!("CARGO_PKG_VERSION"));

    fn new() -> Self {
        Self::new(48_000)
    }

    fn window_options() -> WindowOptions {
        WindowOptions::new().with_size((880.0, 560.0))
    }

    fn param_map(&self) -> ParamMap {
        self.engine.param_map()
    }
}

export_clap!(ParticulaProcessor);
