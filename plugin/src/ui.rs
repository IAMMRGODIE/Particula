//! Particula control surface — styled after the HOMOLOGY sigil-system site.
//!
//! The view holds the shared atomic ParamMap + counter Arcs and reads/writes
//! them directly on every frame (the wavetable_synth pattern): sliders write
//! straight into the map in their on_change closure, so nothing depends on the
//! plugin's message routing. Stats and preset cards work the same way.

use std::sync::{
    Arc,
    atomic::Ordering,
};

use iced::{
    Color, Element, Font, Length,
    font::Family,
    widget::{button, canvas, canvas::Path, column, container, row, slider, text},
};

use i_am_dsp::prelude::{AtomicValue, ParamMap, SetValue};

// ------------------------- palette (HOMOLOGY tokens) -------------------------
pub const TEXT: Color = Color::from_rgb(0.93, 0.93, 0.93);
pub const TEXT_DIM: Color = Color::from_rgb(0.60, 0.60, 0.60);
pub const TEXT_FAINT: Color = Color::from_rgb(0.36, 0.36, 0.36);
pub const LINE: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.14);
pub const BG: Color = Color::from_rgb(0.030, 0.030, 0.030);
pub const BG_PANEL: Color = Color::from_rgb(0.046, 0.046, 0.046);
pub const GLOW: Color = Color::from_rgb(0.95, 0.95, 0.95);

pub const MONO: Font = Font::MONOSPACE;
pub const DISPLAY: Font = Font {
    family: Family::Serif,
    ..Font::DEFAULT
};

// ------------------------------- messages -----------------------------------
#[derive(Debug, Clone)]
pub enum ParticulaMessage {
    /// Kept for CLI/structure compatibility; the surface writes the map
    /// directly instead of routing through messages.
    Param { id: &'static str, value: f32 },
    Tick,
}

impl i_am_dsp_iced::Message for ParticulaMessage {
    fn from_note_event(_: i_am_dsp::NoteEvent) -> Self {
        Self::Tick
    }
    fn note_event(&self) -> Option<i_am_dsp::NoteEvent> {
        None
    }
    fn tick(_: iced::time::Instant) -> Self {
        Self::Tick
    }
}

// --------------------------- parameter snapshot ------------------------------
/// One parameter read live from the shared atomic map.
#[derive(Debug, Clone, Copy)]
pub struct ParamSnapshot {
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

/// The parameter ids the surface exposes, grouped into panels.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("01 · Spawn", &["spawn_interval_ms", "max_particles", "reverse_chance"]),
    ("02 · Position", &["lfo_rate_hz", "lfo_depth", "position_smooth_ms"]),
    ("03 · Tone", &["texture_blend", "texture_stretch", "pitch_max"]),
    ("04 · Output", &["feedback_gain", "feedback_delay_ms", "wet"]),
];

/// Short display names for the exposed parameter ids.
pub const LABELS: &[(&str, &str)] = &[
    ("spawn_interval_ms", "Interval"),
    ("max_particles", "Pool"),
    ("reverse_chance", "Reverse"),
    ("lfo_rate_hz", "LFO Rate"),
    ("lfo_depth", "LFO Depth"),
    ("position_smooth_ms", "Smooth"),
    ("texture_blend", "Texture"),
    ("texture_stretch", "Stretch"),
    ("pitch_max", "Pitch"),
    ("feedback_gain", "Feedback"),
    ("feedback_delay_ms", "FB Delay"),
    ("wet", "Wet"),
];

pub fn label(id: &str) -> &str {
    LABELS
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| *v)
        .unwrap_or(id)
}

/// Presets: clickable constellation cards (HOMOLOGY .preset).
pub const PRESETS: &[(&str, &[(&str, f32)])] = &[
    (
        "Halo Chamber",
        &[
            ("wet", 0.95),
            ("feedback_gain", 0.5),
            ("texture_blend", 0.7),
            ("texture_stretch", 0.7),
            ("reverse_chance", 0.15),
        ],
    ),
    (
        "Reverse Rain",
        &[
            ("reverse_chance", 0.85),
            ("spawn_interval_ms", 25.0),
            ("lifetime_ms_max", 900.0),
            ("feedback_gain", 0.35),
            ("lfo_depth", 0.3),
        ],
    ),
    (
        "Metal Swarm",
        &[
            ("pitch_max", 2.2),
            ("freq_shift_min", -400.0),
            ("freq_shift_max", 400.0),
            ("spawn_interval_ms", 14.0),
            ("wet", 1.0),
            ("dry", 0.25),
        ],
    ),
    (
        "Deep Drift",
        &[
            ("texture_blend", 0.9),
            ("texture_stretch", 0.45),
            ("lfo_depth", 0.1),
        ],
    ),
];

/// Reads one parameter out of the shared atomic map.
pub fn snapshot(id: &'static str, map: &ParamMap) -> Option<ParamSnapshot> {
    let av = map.get(id)?;
    let range = match &*av {
        AtomicValue::Float { range, .. } => (*range.start(), *range.end()),
        AtomicValue::Int { range, .. } => (*range.start() as f32, *range.end() as f32),
        AtomicValue::Bool { .. } => (0.0, 1.0),
        _ => (0.0, 0.0),
    };
    let value = match av.load(Ordering::Relaxed) {
        SetValue::Float(v) => v,
        SetValue::Int(v) => v as f32,
        SetValue::Bool(v) => {
            if v {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    Some(ParamSnapshot {
        value,
        min: range.0,
        max: range.1,
    })
}

/// The control surface. Owns Arc handles into the shared state; every read is
/// live, every write lands in the atomic map immediately.
pub struct ParticulaView {
    pub param_map: ParamMap,
    pub stats: Arc<[std::sync::atomic::AtomicUsize; 3]>,
}

impl i_am_dsp_iced::SyncedView for ParticulaView {
    type Message = ParticulaMessage;

    fn update(&mut self, _: &Self::Message) {}

    fn view(&self) -> Element<'_, Self::Message> {
        Self::build(self)
    }
}

impl ParticulaView {
    fn val(&self, id: &'static str) -> Option<ParamSnapshot> {
        snapshot(id, &self.param_map)
    }

    fn live(&self) -> usize {
        self.stats[0].load(Ordering::Relaxed)
    }
    fn spawned(&self) -> usize {
        self.stats[1].load(Ordering::Relaxed)
    }
    fn sample_rate(&self) -> usize {
        self.stats[2].load(Ordering::Relaxed)
    }

    fn build(&self) -> Element<'static, ParticulaMessage> {
        let header = container(
            row![
                sigil_mark(26.0),
                text("P A R T I C U L A").font(DISPLAY).size(15).color(TEXT),
                iced::widget::space(),
                text("●  S Y S T E M   L I V E")
                    .font(MONO)
                    .size(9)
                    .color(TEXT),
            ]
            .align_y(iced::Alignment::Center)
            .padding([0, 4]),
        )
        .padding([12, 24])
        .style(panel_style(None, LINE, 0.0, 0.0));

        let sigil_column = column![
            text("THE SIGIL SYSTEM · GRAIN CLOUD")
                .font(MONO)
                .size(9)
                .color(TEXT_FAINT),
            text("Particula").font(DISPLAY).size(34).color(TEXT),
            text("one shared history,
read as a constellation of voices.")
                .font(MONO)
                .size(10)
                .color(TEXT_DIM),
            iced::widget::canvas(SigilMatrix {
                lit: self.live().min(30),
                cols: 6,
                rows: 5,
            })
            .width(Length::Fixed(150.0))
            .height(Length::Fixed(96.0)),
            text(format!("LIVE  {}  /  POOL  256", self.live()))
                .font(MONO)
                .size(8)
                .color(TEXT_FAINT),
        ]
        .spacing(10)
        .padding([0, 4]);

        let group_columns: Vec<Element<'static, ParticulaMessage>> = GROUPS
            .chunks(2)
            .map(|pair| {
                let cols = pair
                    .iter()
                    .map(|(title, ids)| self.panel(title, ids))
                    .collect::<Vec<_>>();
                row(cols).spacing(18).into()
            })
            .collect();

        let groups = column(group_columns).spacing(14).padding([0, 4]);

        let middle = row![sigil_column, groups].spacing(32).padding([22, 24]);

        // Preset constellation cards (homology .preset with a mini-matrix).
        let preset_row = row(
            PRESETS
                .iter()
                .map(|(name, params)| self.preset_card(name, params))
                .collect::<Vec<_>>(),
        )
        .spacing(12)
        .padding([0, 24]);

        let stats = container(
            row![
                stat_pill(self.live(), "LIVE"),
                stat_pill(self.spawned(), "SPAWNED"),
                stat_pill(self.sample_rate(), "SAMPLE RATE"),
                stat_pill(3, "SIGIL"),
            ]
            .padding([14, 24]),
        )
        .style(panel_style(Some(BG_PANEL), LINE, 1.0, 0.0));

        container(column![header, middle, preset_row, stats].spacing(0))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(panel_style(Some(BG), Color::TRANSPARENT, 0.0, 0.0))
            .into()
    }

    /// One panel: kicker title + a vertical stack of live parameter rows.
    fn panel(&self, title: &'static str, ids: &'static [&'static str]) -> Element<'static, ParticulaMessage> {
        let rows = ids
            .iter()
            .map(|id| self.param_row(id))
            .collect::<Vec<_>>();

        column![
            row![
                text(title).font(MONO).size(9).color(TEXT_DIM),
                iced::widget::space(),
                iced::widget::canvas(SigilPips {
                    lit: self.group_lit(ids),
                    cols: 4,
                    rows: 2,
                })
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(20.0)),
            ]
            .align_y(iced::Alignment::Center),
            column(rows).spacing(2),
        ]
        .spacing(10)
        .into()
    }

    /// Fractional lit count across a panel, scaled to 8 pip positions.
    fn group_lit(&self, ids: &'static [&'static str]) -> usize {
        let mut sum = 0.0_f32;
        let mut n = 0usize;
        for id in ids {
            if let Some(s) = self.val(id) {
                let norm = if s.max > s.min {
                    ((s.value - s.min) / (s.max - s.min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                sum += norm;
                n += 1;
            }
        }
        if n == 0 {
            0
        } else {
            ((sum / n as f32) * 8.0).round() as usize
        }
    }

    fn param_row(&self, id: &'static str) -> Element<'static, ParticulaMessage> {
        let Some(snap) = self.val(id) else {
            return iced::widget::space().into();
        };
        let map = self.param_map.clone();
        let id_for_event = id;

        container(
            row![
                text(label(id))
                    .font(MONO)
                    .size(9)
                    .color(TEXT_DIM)
                    .width(Length::Fixed(72.0)),
                slider(snap.min..=snap.max, snap.value, move |v| {
                    // Write the atomic map directly: the audio thread applies
                    // it via Paramed::sync_params on the next sample.
                    map.set(id_for_event, v, Ordering::Relaxed);
                    ParticulaMessage::Tick
                })
                .step(nice_step(snap.min, snap.max))
                .style(slider_style),
                text(format!("{:.2}", self.val(id).map(|s| s.value).unwrap_or(0.0)))
                    .font(MONO)
                    .size(9)
                    .color(TEXT_FAINT)
                    .width(Length::Fixed(52.0)),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(8),
        )
        .padding([7, 2])
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }

    /// A clickable preset card: applying the constellation to the map.
    fn preset_card(
        &self,
        name: &'static str,
        params: &'static [(&'static str, f32)],
    ) -> Element<'static, ParticulaMessage> {
        let map = self.param_map.clone();
        button(
            column![
                iced::widget::canvas(SigilMatrix {
                    lit: 7,
                    cols: 4,
                    rows: 4,
                })
                .width(Length::Fixed(72.0))
                .height(Length::Fixed(60.0)),
                text(name).font(MONO).size(9).color(TEXT),
            ]
            .spacing(6)
            .align_x(iced::Alignment::Center),
        )
        .on_press_with(move || {
            for (pid, pv) in params {
                map.set(pid, *pv, Ordering::Relaxed);
            }
            ParticulaMessage::Tick
        })
        .style(preset_button_style)
        .into()
    }
}

// ------------------------------ viewers -------------------------------------
fn stat_pill(n: usize, l: &'static str) -> Element<'static, ParticulaMessage> {
    column![
        text(n.to_string()).font(DISPLAY).size(19).color(TEXT),
        text(l).font(MONO).size(8).color(TEXT_FAINT),
    ]
    .spacing(2)
    .width(Length::Fill)
    .align_x(iced::Alignment::Center)
    .into()
}

fn panel_style(
    background: Option<Color>,
    border_color: Color,
    border_width: f32,
    radius: f32,
) -> impl Fn(&iced::Theme) -> container::Style + 'static {
    move |_: &iced::Theme| container::Style {
        background: background.map(iced::Background::Color),
        border: iced::Border {
            color: border_color,
            width: border_width,
            radius: radius.into(),
            ..Default::default()
        },
        ..container::Style::default()
    }
}

fn slider_style(_: &iced::Theme, _: slider::Status) -> slider::Style {
    use iced::widget::slider::{Handle, HandleShape, Rail};
    slider::Style {
        rail: Rail {
            backgrounds: (
                iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.28)),
                iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.10)),
            ),
            width: 2.0,
            border: iced::Border::default(),
        },
        handle: Handle {
            shape: HandleShape::Circle { radius: 5.0 },
            background: iced::Background::Color(TEXT),
            border_width: 0.0,
            border_color: iced::Color::TRANSPARENT,
        },
    }
}

fn preset_button_style(_: &iced::Theme, status: button::Status) -> button::Style {
    let (border_color, text_color) = match status {
        button::Status::Hovered | button::Status::Pressed => (LINE, TEXT),
        _ => (Color::from_rgba(1.0, 1.0, 1.0, 0.10), TEXT_DIM),
    };
    button::Style {
        background: Some(iced::Background::Color(BG_PANEL)),
        text_color,
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: 2.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

// ------------------------------- canvas bits --------------------------------
/// Header sigil: ring + diagonals + apex dot (HOMOLOGY mark).
struct SigilMark {
    size: f32,
}

fn sigil_mark(size: f32) -> Element<'static, ParticulaMessage> {
    iced::widget::canvas(SigilMark { size })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

impl<M> canvas::Program<M> for SigilMark {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use iced::widget::canvas::Stroke;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let c = frame.center();
        let r = self.size * 0.42;
        frame.stroke(
            &Path::circle(c, r),
            Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.5)),
                ..Stroke::default()
            },
        );
        let top = iced::Point::new(c.x, c.y - r);
        frame.stroke(
            &Path::line(top, iced::Point::new(c.x - r * 0.72, c.y + r * 0.68)),
            Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.9)),
                ..Stroke::default()
            },
        );
        frame.stroke(
            &Path::line(top, iced::Point::new(c.x + r * 0.72, c.y + r * 0.68)),
            Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.9)),
                ..Stroke::default()
            },
        );
        frame.fill(&Path::circle(top, 1.6), GLOW);
        vec![frame.into_geometry()]
    }
}

/// Dot matrix.
struct SigilMatrix {
    lit: usize,
    cols: usize,
    rows: usize,
}

impl<M> canvas::Program<M> for SigilMatrix {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let step_x = bounds.width / self.cols as f32;
        let step_y = bounds.height / self.rows as f32;
        let dot = (step_x.min(step_y) * 0.12).max(1.2);
        let mut i = 0usize;
        for r in 0..self.rows {
            for c in 0..self.cols {
                let center = iced::Point::new((c as f32 + 0.5) * step_x, (r as f32 + 0.5) * step_y);
                let color = if i < self.lit {
                    GLOW
                } else {
                    Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                };
                frame.fill(&Path::circle(center, dot), color);
                i += 1;
            }
        }
        vec![frame.into_geometry()]
    }
}

/// A tidy, range-adaptive slider step: target ~200 notches across the
/// parameter span (iced defaults to step = 1, which quantizes e.g. a 0..1
/// blend slider to two positions).
fn nice_step(min: f32, max: f32) -> f32 {
    let span = (max - min).abs();
    if span <= 0.0 {
        return 0.0;
    }
    let raw = span / 200.0;
    let base = 10.0_f32.powf(raw.log10().floor());
    let m = raw / base;
    let nice = if m < 1.5 {
        1.0
    } else if m < 3.0 {
        2.0
    } else if m < 7.0 {
        5.0
    } else {
        10.0
    };
    (nice * base).max(1e-4)
}

/// Tiny 4x2 pip row for a panel header (value-scaled).
struct SigilPips {
    lit: usize,
    cols: usize,
    rows: usize,
}

impl<M> canvas::Program<M> for SigilPips {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let step_x = bounds.width / self.cols as f32;
        let step_y = bounds.height / self.rows as f32;
        let dot = (step_x.min(step_y) * 0.14).max(1.0);
        let mut i = 0usize;
        for r in 0..self.rows {
            for c in 0..self.cols {
                let center = iced::Point::new((c as f32 + 0.5) * step_x, (r as f32 + 0.5) * step_y);
                let color = if i < self.lit {
                    GLOW
                } else {
                    Color::from_rgba(1.0, 1.0, 1.0, 0.07)
                };
                frame.fill(&Path::circle(center, dot), color);
                i += 1;
            }
        }
        vec![frame.into_geometry()]
    }
}
