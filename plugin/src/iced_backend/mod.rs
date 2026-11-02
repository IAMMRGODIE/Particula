//! An iced backend for this plug-in.
//!
//! `i_am_dsp`'s workspace used to carry `i_am_dsp_iced` and `i_am_plugin_iced`
//! (the widget kit and the iced `Editor`). It dropped both when the rest of the
//! family moved to WebView pages, which left this plug-in without its GUI even
//! though `i_am_plugin` still has the toolkit-neutral `Editor` seam.
//!
//! This module is the small piece of those crates Particula actually used: the
//! `SyncedView` / `Processor` pair the view is written against, the theme it is
//! drawn in, and [IcedEditor], which plugs an iced program into the seam. Keeping
//! it here means i_am_dsp can stay iced-free while Particula keeps its own GUI.

use iced::Element;

mod editor;
mod styles;

pub use editor::IcedEditor;
pub use styles::ERROR_COLOR;
pub use styles::theme;

/// The message trait of `i_am_dsp`, re-exported so the view and the editor name
/// the same thing.
pub use i_am_dsp::Message;

/// A view that can be synced with its processor.
pub trait SyncedView: Send {
    /// The message type used by the view.
    type Message;

    /// Applies a message to the view.
    fn update(&mut self, message: &Self::Message);

    /// The view function for the iced gui.
    fn view(&self) -> Element<'_, Self::Message>;
}

/// A processor with a view attached.
///
/// It is a processor from `i_am_dsp` plus the view that draws it.
pub trait Processor: i_am_dsp::Processor {
    /// The view type used by the processor.
    type SyncedView: SyncedView<Message = Self::Message>;

    /// The view for the processor.
    fn synced_view(&self) -> Self::SyncedView;
}
