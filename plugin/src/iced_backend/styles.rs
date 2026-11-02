//! The dark theme Particula is drawn in.
//!
//! Trimmed from the theme `i_am_dsp_iced` shipped: only the colours and the
//! palette this plug-in reads.

use iced::{
    Color, Theme,
    theme::palette::{
        Background, Danger, Extended, Pair, Primary, Secondary, Success, Warning, mix,
    },
};

/// The window background.
pub static BACKGROUND_COLOR: Color = Color::from_rgba(
    0x1E as f32 / 255.0,
    0x1E as f32 / 255.0,
    0x1E as f32 / 255.0,
    1.0,
);
/// The background of a card.
pub static CARD_COLOR: Color = Color::from_rgba(
    0x27 as f32 / 255.0,
    0x27 as f32 / 255.0,
    0x27 as f32 / 255.0,
    1.0,
);
/// The border of a card.
pub static CARD_BORDER_COLOR: Color = Color::from_rgba(
    0x3D as f32 / 255.0,
    0x3D as f32 / 255.0,
    0x3D as f32 / 255.0,
    1.0,
);

/// The primary colour for the application.
pub static PRIMARY_COLOR: Color = Color::from_rgba(
    0x8A as f32 / 255.0,
    0x6A as f32 / 255.0,
    0xFF as f32 / 255.0,
    1.0,
);
/// The secondary colour for the application.
pub static SECONDARY_COLOR: Color = Color::from_rgba(
    0x6B as f32 / 255.0,
    0xB5 as f32 / 255.0,
    0xFF as f32 / 255.0,
    1.0,
);

/// The default colour for an error message.
pub static ERROR_COLOR: Color = Color::from_rgba(
    0xFF as f32 / 255.0,
    0x4D as f32 / 255.0,
    0x6D as f32 / 255.0,
    1.0,
);
/// The default colour for a success message.
pub static SUCCESS_COLOR: Color = Color::from_rgba(
    0x00 as f32 / 255.0,
    0xC8 as f32 / 255.0,
    0x97 as f32 / 255.0,
    1.0,
);
/// The default colour for a warning message.
pub static WARNING_COLOR: Color = Color::from_rgba(
    0xFF as f32 / 255.0,
    0xB8 as f32 / 255.0,
    0x5C as f32 / 255.0,
    1.0,
);

/// The default title colour.
pub static PRIMARY_TEXT_COLOR: Color = Color::from_rgba(
    0xE0 as f32 / 255.0,
    0xE0 as f32 / 255.0,
    0xE0 as f32 / 255.0,
    1.0,
);
/// The default text colour.
pub static SECONDARY_TEXT_COLOR: Color = Color::from_rgba(
    0xB0 as f32 / 255.0,
    0xB0 as f32 / 255.0,
    0xB0 as f32 / 255.0,
    1.0,
);

/// The colour a faceplate draws itself in when it has no identity colour.
pub const IDENTITY_COLOR: Color = PRIMARY_COLOR;

/// The default theme for the application.
pub fn theme() -> Theme {
    theme_with_accent(IDENTITY_COLOR)
}

/// The default theme, with the primary colour replaced by an effect's own.
pub fn theme_with_accent(accent: Color) -> Theme {
    let palette = iced::theme::Palette {
        background: CARD_COLOR,
        text: PRIMARY_TEXT_COLOR,
        primary: accent,
        success: SUCCESS_COLOR,
        warning: WARNING_COLOR,
        danger: ERROR_COLOR,
    };

    Theme::custom_with_fn("Nablo", palette, move |_| Extended {
        is_dark: true,
        success: Success::generate(SUCCESS_COLOR, BACKGROUND_COLOR, PRIMARY_TEXT_COLOR),
        warning: Warning::generate(WARNING_COLOR, BACKGROUND_COLOR, PRIMARY_TEXT_COLOR),
        danger: Danger::generate(ERROR_COLOR, BACKGROUND_COLOR, PRIMARY_TEXT_COLOR),
        secondary: Secondary::generate(SECONDARY_COLOR, SECONDARY_TEXT_COLOR),
        primary: Primary::generate(accent, BACKGROUND_COLOR, PRIMARY_TEXT_COLOR),

        background: Background {
            base: Pair::new(CARD_COLOR, PRIMARY_TEXT_COLOR),
            weakest: Pair::new(BACKGROUND_COLOR, SECONDARY_TEXT_COLOR),
            weaker: Pair::new(mix(CARD_COLOR, BACKGROUND_COLOR, 0.3), SECONDARY_TEXT_COLOR),
            weak: Pair::new(mix(CARD_COLOR, BACKGROUND_COLOR, 0.7), SECONDARY_TEXT_COLOR),
            neutral: Pair::new(CARD_COLOR, PRIMARY_TEXT_COLOR),
            strong: Pair::new(mix(CARD_COLOR, CARD_BORDER_COLOR, 0.3), PRIMARY_TEXT_COLOR),
            stronger: Pair::new(mix(CARD_COLOR, CARD_BORDER_COLOR, 0.7), PRIMARY_TEXT_COLOR),
            strongest: Pair::new(CARD_BORDER_COLOR, PRIMARY_TEXT_COLOR),
        },
    })
}
