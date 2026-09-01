//! Particula control surface — a living HOMOLOGY sigil.
//!
//! Animation structure:
//!  - The engine pushes a SpawnEvent per born particle over a crossbeam
//!    channel. The view drains it on every tick, allocating a circular dot
//!    slot (three concentric rings, 12 slots each) and fading its brightness
//!    as the particle ages toward its lifetime.
//!  - Rings rotate at different angular speeds (differential rotation).
//!  - Clicking the left/right half of the sigil toggles a parameter panel that
//!    fades in/out via a per-frame lerp.
//!  - A header About overlay and a footer randomize button are plain state.
//! All reads/writes go through the shared atomic ParamMap + a spawn-event
//! channel — never shared mutable state with the audio thread.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use iced::{
    Color, Element, Font, Length,
    font::Family,
    widget::{button, canvas, column, container, row, slider, text},
};
use i_am_dsp::prelude::{AtomicValue, ParamMap, SetValue};

use particula::SpawnEvent;

// ------------------------------ constants -----------------------------------
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

/// Number of concentric rings on the sigil.
const RINGS: usize = 3;
/// Dots on each ring (a circular slot buffer per ring).
const DOTS_PER_RING: usize = 12;
/// Angular velocities of each ring (radians / second), inner ring fastest.
const RING_SPEED: [f32; RINGS] = [0.45, 0.22, -0.12];
/// Fade-in/out time constant (per second, larger = snappier).
const PANEL_TAU: f32 = 7.0;

// ------------------------------- messages -----------------------------------
#[derive(Debug, Clone)]
pub enum ParticulaMessage {
    Param { id: &'static str, value: f32 },
    ToggleLeft,
    ToggleRight,
    ShowAbout(bool),
    Randomize,
    MasterEnabled(bool),
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
#[derive(Debug, Clone, Copy)]
pub struct ParamSnapshot {
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

pub fn snapshot(id: &'static str, map: &ParamMap) -> Option<ParamSnapshot> {
    let av = map.get(id)?;
    let range = match &*av {
        AtomicValue::Float { range, .. } => (*range.start(), *range.end()),
        AtomicValue::Int { range, .. } => (*range.start() as f32, *range.end() as f32),
        AtomicValue::Bool { .. } => (0.0, 1.0),
        _ => (0.0, 0.0),
    };
    let value = match av.load(std::sync::atomic::Ordering::Relaxed) {
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

/// Short label for an exposed parameter.
pub fn label(id: &str) -> String {
    let name = match id {
        "spawn_interval_ms" => "Interval",
        "max_particles" => "Pool",
        "reverse_chance" => "Reverse",
        "lfo_rate_hz" => "LFO Rate",
        "lfo_depth" => "LFO Depth",
        "position_smooth_ms" => "Smooth",
        "texture_blend" => "Texture",
        "texture_stretch" => "Stretch",
        "pitch_max" => "Pitch",
        "feedback_gain" => "Feedback",
        "feedback_delay_ms" => "FB Delay",
        "base_position" => "Base Pos",
        "wet" => "Wet",
        "dry" => "Dry",
        _ => id,
    };
    name.to_string()
}

/// Leaves, particles, panels: parameters shown in each side panel.
const LEFT_PARAMS: &[&'static str] =
    &["spawn_interval_ms", "max_particles", "reverse_chance", "base_position"];
const RIGHT_PARAMS: &[&'static str] =
    &["texture_blend", "feedback_gain", "lfo_depth", "position_smooth_ms"];

// --------------------------------- dots -------------------------------------
/// One lit dot on the sigil: a particle, fading over its lifetime.
#[derive(Debug, Clone, Copy)]
struct Dot {
    /// Age in seconds since it lit up.
    age: f32,
    /// Lifetime in seconds (converted from the spawn event's samples).
    lifetime: f32,
}

/// Fade factor (1 = just born, 0 = dead).
fn dot_alpha(d: &Dot) -> f32 {
    if d.lifetime <= 0.0 || d.age > d.lifetime {
        0.0
    } else {
        1.0 - d.age / d.lifetime
    }
}

/// Per-ring circular slot buffer.
type RingSlots = [Dot; DOTS_PER_RING];

// ------------------------------ panel anim ----------------------------------
/// A tweened panel visibility.
#[derive(Debug, Clone, Copy)]
struct PanelAnim {
    target: bool,
    opacity: f32,
}

impl PanelAnim {
    fn hidden() -> Self {
        Self {
            target: false,
            opacity: 0.0,
        }
    }
    fn update(&mut self, dt: f32) {
        let goal = if self.target { 1.0 } else { 0.0 };
        self.opacity += (goal - self.opacity) * (PANEL_TAU * dt).min(1.0);
        if (goal - self.opacity).abs() < 0.002 {
            self.opacity = goal;
        }
    }
}

// -------------------------------- the view ----------------------------------
/// GUI state: the sigil animation + panels + shared atomic handles.
pub struct ParticulaView {
    pub param_map: ParamMap,
    pub stats: Arc<[std::sync::atomic::AtomicUsize; 3]>,
    /// Spawn events posted by the audio thread, drained on every tick.
    spawn_rx: Arc<Mutex<Receiver<SpawnEvent>>>,

    /// Lit dots per ring (circular slot buffers).
    dots: [RingSlots; RINGS],
    /// Global dot slot pointer (rotates across rings).
    next_dot: usize,
    /// Differential ring rotation angles (radians).
    ring_phases: [f32; RINGS],

    /// Panel fade animation.
    panel_left: PanelAnim,
    panel_right: PanelAnim,
    /// Whether the About overlay is open.
    about: bool,

    last_frame: Option<Instant>,
}

impl i_am_dsp_iced::SyncedView for ParticulaView {
    type Message = ParticulaMessage;

    fn update(&mut self, message: &Self::Message) {
        match message {
            ParticulaMessage::Tick => {
                let now = Instant::now();
                let dt = self
                    .last_frame
                    .map(|t| now.duration_since(t).as_secs_f32())
                    .unwrap_or(0.0);
                self.last_frame = Some(now);
                self.animate(dt);
            }
            ParticulaMessage::ToggleLeft => self.panel_left.target = !self.panel_left.target,
            ParticulaMessage::ToggleRight => self.panel_right.target = !self.panel_right.target,
            ParticulaMessage::ShowAbout(show) => self.about = *show,
            ParticulaMessage::Randomize => randomize_all(&self.param_map),
            ParticulaMessage::MasterEnabled(v) => {
                self.param_map.set("enabled", *v, std::sync::atomic::Ordering::Relaxed);
            }
            ParticulaMessage::Param { .. } => {}
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        Self::build(self)
    }
}

impl ParticulaView {
    /// Builds the view from the shared audio/GUI handles.
    pub fn new(
        param_map: ParamMap,
        stats: Arc<[std::sync::atomic::AtomicUsize; 3]>,
        spawn_rx: Arc<Mutex<Receiver<SpawnEvent>>>,
    ) -> Self {
        Self {
            param_map,
            stats,
            spawn_rx,
            dots: [[Dot {
                age: 0.0,
                lifetime: 0.0,
            }; DOTS_PER_RING]; RINGS],
            next_dot: 0,
            ring_phases: [0.0; RINGS],
            panel_left: PanelAnim::hidden(),
            panel_right: PanelAnim::hidden(),
            about: false,
            last_frame: None,
        }
    }

    /// Advance the animation by dt: ingest spawn events, fade dots, spin rings,
    /// tween panels.
    fn animate(&mut self, dt: f32) {
        // 1. Ingest any spawn events posted by the audio thread.
        {
            let rx = self.spawn_rx.lock().expect("spawn receiver lock");
            for ev in rx.try_iter() {
                let ring = self.next_dot % RINGS;
                let slot = (self.next_dot / RINGS) % DOTS_PER_RING;
                let sr = if self.stats[2].load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    48_000.0
                } else {
                    self.stats[2].load(std::sync::atomic::Ordering::Relaxed) as f32
                };
                self.dots[ring][slot] = Dot {
                    age: 0.0,
                    lifetime: ev.lifetime_samples as f32 / sr,
                };
                self.next_dot += 1;
            }
        }

        // 2. Age the dots (keep them until the slot is reused by a new spawn).
        for ring in 0..RINGS {
            for slot in 0..DOTS_PER_RING {
                self.dots[ring][slot].age += dt;
            }
        }

        // 3. Rotate the rings.
        for (r, phase) in self.ring_phases.iter_mut().enumerate() {
            *phase += RING_SPEED[r] * dt;
        }

        // 4. Tween the panels.
        self.panel_left.update(dt);
        self.panel_right.update(dt);
    }

    fn live(&self) -> usize {
        self.stats[0].load(std::sync::atomic::Ordering::Relaxed)
    }
    fn spawned(&self) -> usize {
        self.stats[1].load(std::sync::atomic::Ordering::Relaxed)
    }
    fn sample_rate(&self) -> usize {
        self.stats[2].load(std::sync::atomic::Ordering::Relaxed)
    }

    fn build(&self) -> Element<'static, ParticulaMessage> {
        // ---- header ----
        let header = container(
            row![
                button(sigil_mark(26.0))
                    .on_press(ParticulaMessage::ShowAbout(true))
                    .style(flat_button),
                text("P A R T I C U L A").font(DISPLAY).size(15).color(TEXT),
                iced::widget::space(),
                text("WET").font(MONO).size(8).color(TEXT_FAINT),
                self.mini_slider("wet"),
                text("DRY").font(MONO).size(8).color(TEXT_FAINT),
                self.mini_slider("dry"),
                self.enabled_toggler(),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(10),
        )
        .padding([12, 24])
        .style(panel_style(None, LINE, 0.0, 0.0));

        // ---- centre sigil: canvas + left/right click zones (half layout) ----
        let sigil = iced::widget::stack![
            iced::widget::canvas(SigilCanvas {
                dots: self.dots,
                phases: self.ring_phases,
            })
            .width(Length::Fill)
            .height(Length::Fill),
            row![
                container(
                    iced::widget::mouse_area(
                        iced::widget::space().width(Length::Fill).height(Length::Fill)
                    )
                    .on_press(ParticulaMessage::ToggleLeft)
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill),
                container(
                    iced::widget::mouse_area(
                        iced::widget::space().width(Length::Fill).height(Length::Fill)
                    )
                    .on_press(ParticulaMessage::ToggleRight)
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        let centre = container(sigil)
            .width(Length::Fill)
            .height(Length::Fill);

        // ---- side panels (faded in/out) ----
        let left = self.side_panel("01 · GENERATION", LEFT_PARAMS, self.panel_left);
        let right = self.side_panel("02 · MATERIAL / MODULATION", RIGHT_PARAMS, self.panel_right);

        let body = row![left, centre, right].spacing(0);

        // ---- footer ----
        let status_line = format!("{:<5} LIVE   {} SPAWNED   {} HZ", self.live(), self.spawned(), self.sample_rate());
        let footer = container(
            row![
                text(status_line)
                    .font(MONO)
                    .size(9)
                    .color(TEXT_FAINT),
                iced::widget::space(),
                button(text("RANDOMIZE").font(MONO).size(9).color(TEXT_DIM))
                    .on_press(ParticulaMessage::Randomize)
                    .style(flat_button),
            ]
            .align_y(iced::Alignment::Center)
            .padding([0, 24]),
        )
        .style(panel_style(Some(BG_PANEL), LINE, 1.0, 0.0));

        let base = container(column![header, body, footer].spacing(0))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(panel_style(Some(BG), Color::TRANSPARENT, 0.0, 0.0));

        // ---- optional About overlay ----
        if self.about {
            let overlay: Element<'static, ParticulaMessage> = container(
                column![
                    text("PARTICULA").font(DISPLAY).size(30).color(TEXT),
                    text("a granular-cloud signal engine")
                        .font(MONO)
                        .size(10)
                        .color(TEXT_DIM),
                    text("one shared history · feedback · texture · bpm · reverse")
                        .font(MONO)
                        .size(9)
                        .color(TEXT_FAINT),
                    button(text("CLOSE").font(MONO).size(9).color(TEXT))
                        .on_press(ParticulaMessage::ShowAbout(false))
                        .style(flat_button)
                        .padding([6, 18]),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(panel_style(Some(BG), LINE, 1.0, 0.0))
            .into();
            return iced::widget::stack![base, overlay].into();
        }

        base.into()
    }

    /// A compact slider (header dry/wet).
    fn mini_slider(&self, id: &'static str) -> Element<'static, ParticulaMessage> {
        let Some(s) = snapshot(id, &self.param_map) else {
            return iced::widget::space().into();
        };
        let map = self.param_map.clone();
        slider(s.min..=s.max, s.value, move |v| {
            map.set(id, v, std::sync::atomic::Ordering::Relaxed);
            ParticulaMessage::Tick
        })
        .step(nice_step(s.min, s.max))
        .width(Length::Fixed(90.0))
        .style(slider_style)
        .into()
    }

    fn enabled_toggler(&self) -> Element<'static, ParticulaMessage> {
        let on = snapshot("enabled", &self.param_map)
            .map(|s| s.value > 0.5)
            .unwrap_or(true);
        iced::widget::toggler::Toggler::new(on)
            .on_toggle(ParticulaMessage::MasterEnabled)
            .label("ON")
            .into()
    }

    /// One fading side panel with a few parameter rows.
    fn side_panel(
        &self,
        title: &'static str,
        ids: &'static [&'static str],
        anim: PanelAnim,
    ) -> Element<'static, ParticulaMessage> {
        let rows = ids
            .iter()
            .map(|id| self.param_row(id))
            .collect::<Vec<_>>();
        container(
            column![
                text(title).font(MONO).size(10).color(TEXT_DIM),
                iced::widget::space().height(4),
                column(rows).spacing(2),
            ]
            .padding(18),
        )
        .width(Length::Fixed((anim.opacity * 210.0 + 18.0).max(18.0)))
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }

    fn param_row(&self, id: &'static str) -> Element<'static, ParticulaMessage> {
        let Some(snap) = snapshot(id, &self.param_map) else {
            return iced::widget::space().into();
        };
        let map = self.param_map.clone();
        container(
            row![
                text(label(id))
                    .font(MONO)
                    .size(9)
                    .color(TEXT_DIM)
                    .width(Length::Fixed(64.0)),
                slider(snap.min..=snap.max, snap.value, move |v| {
                    map.set(id, v, std::sync::atomic::Ordering::Relaxed);
                    ParticulaMessage::Tick
                })
                .step(nice_step(snap.min, snap.max))
                .style(slider_style),
                text(format!("{:.2}", snap.value))
                    .font(MONO)
                    .size(9)
                    .color(TEXT_FAINT)
                    .width(Length::Fixed(48.0)),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(8),
        )
        .padding([8, 0])
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }
}

// -------------------------------- randomize ---------------------------------
fn randomize_all(map: &ParamMap) {
    use std::sync::atomic::Ordering;
    enum Plan {
        Float(f32),
        Int(i32),
        Bool(bool),
    }
    let mut rng = tiny_rng();
    for i in 0..map.len() {
        let Some(id) = map.query_param_id(i).map(|s| s.to_string()) else {
            continue;
        };
        if id == "dry" || id == "wet" || id == "enabled" {
            continue;
        }
        // Read an owned value plan, then set it (avoids holding a borrow of
        // `map` while calling `map.set`).
        let plan = match &*map.get_by_index(i).expect("param") {
            AtomicValue::Float {
                range,
                logarithmic,
                ..
            } => {
                let (lo, hi) = (*range.start(), *range.end());
                let v = if *logarithmic && lo > 0.0 {
                    lo * (hi / lo).powf(rng())
                } else {
                    lo + (hi - lo) * rng()
                };
                Plan::Float(v)
            }
            AtomicValue::Int { range, .. } => {
                let (lo, hi) = (*range.start(), *range.end());
                let span = (hi - lo) as u32 + 1;
                Plan::Int(lo + (rng() * span as f32) as i32)
            }
            AtomicValue::Bool { .. } => Plan::Bool(rng() > 0.5),
            _ => continue,
        };
        match plan {
            Plan::Float(v) => {
                map.set(&id, v, Ordering::Relaxed);
            }
            Plan::Int(v) => {
                map.set(&id, v, Ordering::Relaxed);
            }
            Plan::Bool(v) => {
                map.set(&id, v, Ordering::Relaxed);
            }
        }
    }
}

fn tiny_rng() -> impl FnMut() -> f32 {
    let mut x: u64 = 0x9E3779B97F4A7C15;
    move || {
        x ^= x << 7;
        x ^= x >> 9;
        (x as f32) * (1.0 / u32::MAX as f32).abs()
    }
}

// -------------------------------- styling -----------------------------------
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

fn flat_button(_: &iced::Theme, status: button::Status) -> button::Style {
    let border = match status {
        button::Status::Hovered => LINE,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: None,
        text_color: TEXT,
        border: iced::Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
            ..Default::default()
        },
        ..Default::default()
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

// -------------------------------- the sigil ---------------------------------
/// The living sigil: rings, lit dots and a radial glow, plus half-click zones.
struct SigilCanvas {
    dots: [RingSlots; RINGS],
    phases: [f32; RINGS],
}

impl<M> canvas::Program<M> for SigilCanvas {
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
        let w = bounds.width;
        let h = bounds.height;
        let c = frame.center();
        let max_r = w.min(h) * 0.5;

        // Radial glow: concentric filled circles with fading alpha.
        for i in (1..=8).rev() {
            let r = max_r * (i as f32 / 8.0);
            let a = (0.02 * i as f32).clamp(0.0, 0.12);
            frame.fill(
                &canvas::Path::circle(c, r),
                Color::from_rgba(1.0, 1.0, 1.0, a),
            );
        }

        // Three rings, differential rotation.
        for (ring, &phase) in self.phases.iter().enumerate() {
            let r = max_r * (0.72 - ring as f32 * 0.18);
            frame.stroke(
                &canvas::Path::circle(c, r),
                canvas::Stroke {
                    width: 1.0,
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.14)),
                    ..canvas::Stroke::default()
                },
            );

            // Dots: faint skeleton + lit spawns.
            for slot in 0..DOTS_PER_RING {
                let base_angle = slot as f32 / DOTS_PER_RING as f32 * std::f32::consts::TAU;
                let angle = base_angle + phase;
                let dot_pos = iced::Point::new(
                    c.x + angle.cos() * r,
                    c.y + angle.sin() * r,
                );
                let d = &self.dots[ring][slot];
                // Only draw faint skeleton dots where no particle is lit.
                let alpha = dot_alpha(d);
                if alpha > 0.02 {
                    frame.fill(
                        &canvas::Path::circle(dot_pos, 2.6),
                        Color::from_rgba(1.0, 1.0, 1.0, alpha),
                    );
                } else {
                    frame.fill(
                        &canvas::Path::circle(dot_pos, 1.2),
                        Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    );
                }
            }
        }

        // Center mark.
        frame.stroke(
            &canvas::Path::circle(c, max_r * 0.12),
            canvas::Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.4)),
                ..canvas::Stroke::default()
            },
        );
        frame.fill(
            &canvas::Path::circle(c, 2.0),
            Color::from_rgba(1.0, 1.0, 1.0, 0.9),
        );

        // Split indicator (faint vertical divider, left/right zones).
        frame.stroke(
            &canvas::Path::line(
                iced::Point::new(c.x, c.y - max_r * 0.4),
                iced::Point::new(c.x, c.y + max_r * 0.4),
            ),
            canvas::Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.08)),
                ..canvas::Stroke::default()
            },
        );

        vec![frame.into_geometry()]
    }

}

fn sigil_mark(size: f32) -> Element<'static, ParticulaMessage> {
    iced::widget::canvas(SigilMark { size })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

/// Small header mark (kept as a simple ring + apex).
struct SigilMark {
    size: f32,
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
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let c = frame.center();
        let r = self.size * 0.4;
        frame.stroke(
            &canvas::Path::circle(c, r),
            canvas::Stroke {
                width: 1.0,
                style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.5)),
                ..canvas::Stroke::default()
            },
        );
        frame.fill(&canvas::Path::circle(c, 1.8), GLOW);
        vec![frame.into_geometry()]
    }
}
