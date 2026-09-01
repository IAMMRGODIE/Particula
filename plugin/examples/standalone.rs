//! Standalone run of the same processor: a plain iced window + cpal audio,
//! no DAW needed.
//!
//! Run: cargo run --release --offline -p particula_plugin --example standalone

use i_am_dsp_iced::{demo::Demo, iced, styles::theme};
use particula_plugin::ParticulaProcessor;

fn main() {
    iced::application(|| Demo::new(ParticulaProcessor::new), Demo::update, Demo::view)
        .subscription(|_| Demo::<ParticulaProcessor>::subscriber())
        .theme(theme())
        .window_size((860.0, 460.0))
        .run()
        .expect("failed to run standalone app");
}