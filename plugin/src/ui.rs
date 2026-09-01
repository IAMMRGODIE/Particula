//! Particula control surface — styled after the HOMOLOGY sigil-system site.
//!
//! Near-black theme, hairline white rules, wide-track caps for display text,
//! monospaced micro-labels, dot-matrix sigils, glow highlights. All live
//! values come from the shared atomic ParamMap (no shared mutable state
//! between the GUI and audio threads), and every slider sends a
//! Param{id,value} message into that map, which Paramed applies per sample on
//! the audio thread.

use std::sync::atomic::Ordering;

use iced::{Color, Element, Font, Length, font::Family, widget::{canvas::{self, Frame, Path, Program, Stroke}, column, container, row, slider, text}};

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
    /// Set one engine parameter via the shared atomic ParamMap.
    Param { id: &'static str, value: f32 },
    /// 16 ms heartbeat from the host timer (keeps the frame refreshing).
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
/// A read-only copy of one engine parameter, taken from the shared atomic
/// ParamMap on the GUI thread (thread-safe by construction).
#[derive(Debug, Clone, Copy)]
pub struct ParamSnapshot {
    pub id: &'static str,
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
        id,
        value,
        min: range.0,
        max: range.1,
    })
}

/// A GUI-thread snapshot of everything the surface displays.
pub struct ParticulaView {
    pub params: Vec<ParamSnapshot>,
    pub live: usize,
    pub spawned: usize,
    pub sample_rate: usize,
}

impl i_am_dsp_iced::SyncedView for ParticulaView {
    type Message = ParticulaMessage;

    fn update(&mut self, _: &Self::Message) {}

    fn view(&self) -> Element<'_, Self::Message> {
        Self::build(self)
    }
}

impl ParticulaView {
    fn slot(&self, id: &str) -> Option<&ParamSnapshot> {
        self.params.iter().find(|p| p.id == id)
    }

    fn build<'a>(&'a self) -> Element<'a, ParticulaMessage> {
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
                lit: self.live.min(30),
                cols: 6,
                rows: 5,
            })
            .width(Length::Fixed(150.0))
            .height(Length::Fixed(96.0)),
            text(format!("LIVE  {}  /  POOL  256", self.live))
                .font(MONO)
                .size(8)
                .color(TEXT_FAINT),
        ]
        .spacing(10)
        .padding([0, 4]);

        // Groups 2 x 2 so the sliders get room to breathe.
        let group_columns: Vec<Element<'a, ParticulaMessage>> = GROUPS
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

        let stats = container(
            row![
                stat_pill(self.live, "LIVE"),
                stat_pill(self.spawned, "SPAWNED"),
                stat_pill(self.sample_rate, "SAMPLE RATE"),
                stat_pill(3, "SIGIL"),
            ]
            .padding([14, 24]),
        )
        .style(panel_style(Some(BG_PANEL), LINE, 1.0, 0.0));

        container(column![header, middle, stats].spacing(0))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(panel_style(Some(BG), Color::TRANSPARENT, 0.0, 0.0))
            .into()
    }

    /// One panel: kicker title + a vertical stack of parameter rows.
    fn panel<'a>(
        &'a self,
        title: &'static str,
        ids: &'static [&'static str],
    ) -> Element<'a, ParticulaMessage> {
        let rows = ids
            .iter()
            .filter_map(|id| self.slot(id).map(|s| (id, s)))
            .map(|(id, snap)| self.param_row(id, snap))
            .collect::<Vec<_>>();

        column![
            text(title).font(MONO).size(9).color(TEXT_DIM),
            column(rows).spacing(2),
        ]
        .spacing(10)
        .into()
    }

    fn param_row<'a>(
        &'a self,
        id: &'static str,
        snap: &'a ParamSnapshot,
    ) -> Element<'a, ParticulaMessage> {
        container(
            row![
                text(label(id))
                    .font(MONO)
                    .size(9)
                    .color(TEXT_DIM)
                    .width(Length::Fixed(72.0)),
                slider(snap.min..=snap.max, snap.value, move |v| {
                    ParticulaMessage::Param { id, value: v }
                })
                .style(slider_style),
                text(format!("{:.2}", snap.value))
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

impl<M> Program<M> for SigilMark {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
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

/// Dot matrix whose lit count follows the live particle count.
struct SigilMatrix {
    lit: usize,
    cols: usize,
    rows: usize,
}

impl<M> Program<M> for SigilMatrix {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
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
