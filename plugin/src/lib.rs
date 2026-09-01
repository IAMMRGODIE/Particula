//! Particula Cloud — CLAP plugin entry + standalone processor.
//!
//! Host parameter automation flows through `Paramed`: the engine exposes a
//! derived `Parameters` table, `Paramed` mirrors it into an atomic `ParamMap`
//! (host-facing) and copies host changes into the engine each sample.
//!
//! Build (offline):
//!   cargo build --release -p particula_plugin
//! then rename target/release/particula_plugin.dll -> ParticulaCloud.clap
//! (your DAW may want the standard .clap suffix).

use i_am_dsp::{
    Effect, ProcessContext,
    prelude::{ParamMap, Parameter, Parameters, Paramed, SetValue},
};
use i_am_dsp_iced::{Processor, SyncedView, iced};
use i_am_plugin::{Descriptor, Plugin, Tag, WindowOptions, export_clap};
use particula::ParticulaEngine;

/// The processor: the particle cloud engine behind the host parameter
/// automation layer.
pub struct ParticulaProcessor(Paramed<ParticulaEngine<2>>);

impl ParticulaProcessor {
    /// Creates a new processor. The engine adapts to the sample rate the
    /// host reports at runtime, so the initial value is just a starting point.
    pub fn new(sample_rate: usize) -> Self {
        Self(Paramed::new(ParticulaEngine::<2>::new(
            1 << 16, // 1.36 s of history at 48 kHz
            sample_rate.max(1),
            0x5EED_FA11,
        )))
    }
}

impl Parameters for ParticulaProcessor {
    fn get_parameters(&self) -> Vec<Parameter> {
        self.0.get_parameters()
    }

    fn set_parameter(&mut self, identifier: &str, value: SetValue) -> bool {
        self.0.set_parameter(identifier, value)
    }
}

/// Minimal placeholder view. A real UI (knobs/sliders over the parameter
/// list) can replace this later without touching the audio path.
pub struct ParticulaView;

impl SyncedView for ParticulaView {
    type Message = ();

    fn update(&mut self, _: &Self::Message) {}

    fn view(&self) -> iced::Element<'_ , Self::Message> {
        use iced::widget::{column, text};
        column![
            text("particula").size(30),
            text("granular cloud engine — v0..v2"),
            text("history / feedback / peak-follow / WSOLA texture / BPM sync"),
            text("UI under construction: tweak parameters from your DAW."),
        ]
        .spacing(8)
        .padding(24)
        .into()
    }
}

impl Processor for ParticulaProcessor {
    type Message = ();
    type SyncedView = ParticulaView;

    fn delay(&self) -> usize {
        self.0.delay()
    }

    fn on_message(&self, _: Self::Message) {}

    fn process(
        &mut self,
        samples: &mut [f32; 2],
        other: &[[f32; 2]],
        process_context: &mut Box<dyn ProcessContext>,
    ) {
        // Single Main input port: `other` (sidechain inputs) stays empty,
        // so no per-sample conversion allocation is needed in the common path.
        if other.is_empty() {
            self.0.process(samples, &[], process_context);
        } else {
            let refs: Vec<&[f32; 2]> = other.iter().collect();
            self.0.process(samples, &refs, process_context);
        }
    }

    fn synced_view(&self) -> Self::SyncedView {
        ParticulaView
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
        WindowOptions::new().with_size((860.0, 460.0))
    }

    fn param_map(&self) -> ParamMap {
        self.0.param_map()
    }
}

export_clap!(ParticulaProcessor);