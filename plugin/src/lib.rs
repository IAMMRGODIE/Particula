//! Particula — CLAP plugin entry + standalone processor.
//!
//! Host parameter automation flows through Paramed: the engine exposes a
//! derived Parameters table, Paramed mirrors it into an atomic ParamMap
//! (host-facing) and copies host changes into the engine each sample. The GUI
//! (ui.rs) reads and writes the same atomic map, so there is no shared mutable
//! state between the GUI and audio threads.
//!
//! Build (offline):
//!   cargo build --release -p particula_plugin
//! then rename target/release/particula_plugin.dll -> Particula.clap

mod ui;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use crossbeam_channel::{Receiver, unbounded};

use i_am_dsp::{
    Effect, ProcessContext,
    prelude::{ParamMap, Parameter, Parameters, Paramed, SetValue},
};
use i_am_dsp_iced::Processor;
use i_am_plugin::{Descriptor, Plugin, Tag, WindowOptions, export_clap};
use particula::{ParticulaEngine, SpawnEvent};

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
    /// Receiving end of the spawn-event channel, handed to the GUI so it can
    /// light up sigil dots as particles are born.
    spawn_rx: Arc<Mutex<Receiver<SpawnEvent>>>,
    /// PANIC latch shared with the GUI (see ParticulaView::Panic).
    panic_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl ParticulaProcessor {
    /// Creates a new processor. The engine adapts to the sample rate the
    /// host reports at runtime, so the initial value is just a starting point.
    pub fn new(sample_rate: usize) -> Self {
        let (tx, rx) = unbounded();
        let mut engine = ParticulaEngine::<2>::new(
            1 << 16, // 1.36 s of history at 48 kHz
            sample_rate.max(1),
            0x5EED_FA11,
        );
        engine.set_spawn_notifier(tx);
        let panic_flag = engine.panic_flag();
        Self {
            engine: Paramed::new(engine),
            stats: Arc::new([AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)]),
            spawn_rx: Arc::new(Mutex::new(rx)),
            panic_flag,
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
            // These are GUI-local (panel/about/randomize); the view handles them.
            _ => {}
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
        ParticulaView::new(
            self.engine.param_map(),
            self.stats.clone(),
            self.spawn_rx.clone(),
            self.panic_flag.clone(),
        )
    }
}

impl Plugin for ParticulaProcessor {
    const DESCRIPTOR: Descriptor = Descriptor::new("dev.particula", "Particula")
        .with_tags(&[Tag::AudioEffect, Tag::Granular])
        .with_vendor("I Am Plugins")
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

    // Crimson Text (OFL, free for commercial use) embedded so the GUI can use
    // it as the display serif without depending on the host system fonts.
    const EMBEDDED_FONT: Option<&'static [u8]> =
        Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../CrimsonText-Regular-5.ttf"
        )));
}

export_clap!(ParticulaProcessor);
