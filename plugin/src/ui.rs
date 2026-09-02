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
//!  - All reads/writes go through the shared atomic ParamMap + a spawn-event
//!    channel — never shared mutable state with the audio thread.

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

use particula::{SpawnEvent, SplitMix64};

// ------------------------------ constants -----------------------------------
pub const TEXT: Color = Color::from_rgb(0.93, 0.93, 0.93);
pub const TEXT_DIM: Color = Color::from_rgb(0.60, 0.60, 0.60);
pub const TEXT_FAINT: Color = Color::from_rgb(0.36, 0.36, 0.36);
pub const LINE: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.14);
pub const BG: Color = Color::from_rgb(0.030, 0.030, 0.030);
pub const BG_PANEL: Color = Color::from_rgb(0.046, 0.046, 0.046);
pub const GLOW: Color = Color::from_rgb(0.95, 0.95, 0.95);

pub const MONO: Font = Font::MONOSPACE;
/// Embedded Crimson Text (loaded by the plugin's EMBEDDED_FONT hook).
pub const DISPLAY: Font = Font {
    family: Family::Name("Crimson Text"),
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
    /// Double-clicking a slider restores its factory default.
    ParamReset { id: &'static str },
    ToggleLeft,
    ToggleRight,
    PanelPage { side: usize, page: usize },
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
        SetValue::Bool(v) => v as u8 as f32,
        _ => 0.0,
    };
    Some(ParamSnapshot {
        value,
        min: range.0,
        max: range.1,
    })
}

/// Parameters whose slider should use a logarithmic scale (all have min > 0).
const LOG_PARAMS: &[&str] = &[
    "spawn_interval_ms",
    "spawn_interval_beats",
    "lifetime_ms_min",
    "lifetime_ms_max",
    "fallback_bpm",
    "lfo_rate_hz",
    "position_smooth_ms",
    "texture_window_ms",
    "texture_refresh_ms",
    "texture_crossfade_ms",
    "texture_stretch",
    "pitch_min",
    "pitch_max",
    "gain_decay_ratio",
    "dry",
    "wet",
    "random_walk_interval_ms",
    "peak_window_ms",
    "peak_update_ms",
];

fn log_param(id: &str) -> bool {
    LOG_PARAMS.contains(&id)
}

/// Maps (min, max, value) to a slider domain. Log parameters work in
/// ln(domain); zero-min ranges use a tiny epsilon floor so 0 stays reachable.
fn slider_domain(id: &str, min: f32, max: f32, value: f32) -> (f32, f32, f32, bool) {
    if log_param(id) {
        let safe_min = if min > 0.0 { min } else { 1e-3 };
        let safe_max = max.max(safe_min * 2.0);
        let v = value.clamp(safe_min, safe_max);
        (safe_min.ln(), safe_max.ln(), v.ln(), true)
    } else {
        (min, max, value.clamp(min, max), false)
    }
}

/// Whether a parameter row shows based on mode / spawn-sync conditions.
fn cond_matches(cond: u8, mode: usize, sync_on: bool) -> bool {
    match cond {
        0 => true,
        1..=4 => usize::from(cond) == mode + 1,
        5 => sync_on,
        6 => !sync_on,
        _ => false,
    }
}

/// Human names for integer parameters (falls back to numeric 0..n).
fn discrete_options(id: &str) -> &'static [&'static str] {
    match id {
        "position_mode" => &["Fixed", "LFO", "Walk", "Peak"],
        _ => &["0", "1", "2", "3", "4", "5", "6", "7"],
    }
}

/// Short label for an exposed parameter.
pub fn label(id: &str) -> String {
    let name = match id {
        "spawn_interval_ms" => "Interval",
        "spawn_sync" => "Sync",
        "position_mode" => "Mode",
        "max_particles" => "Pool",
        "reverse_chance" => "Reverse",
        "random_walk_step" => "Walk Step",
        "random_walk_interval_ms" => "Walk Int",
        "peak_window_ms" => "Peak Win",
        "peak_update_ms" => "Peak Upd",
        "peak_threshold" => "Peak Thr",
        "spawn_interval_beats" => "Beats",
        "fallback_bpm" => "Fallback",
        "lfo_rate_hz" => "LFO Rate",
        "lfo_depth" => "LFO Depth",
        "position_smooth_ms" => "Smooth",
        "texture_blend" => "Texture",
        "texture_window_ms" => "Window",
        "texture_refresh_ms" => "Refresh",
        "texture_stretch" => "Stretch",
        "texture_crossfade_ms" => "Fade",
        "pitch_min" => "Pitch Min",
        "pitch_max" => "Pitch Max",
        "freq_shift_min" => "Shift Min",
        "freq_shift_max" => "Shift Max",
        "feedback_gain" => "Feedback",
        "feedback_delay_ms" => "FB Delay",
        "feedback_damping_hz" => "FB Damp",
        "lifetime_ms_min" => "Life Min",
        "lifetime_ms_max" => "Life Max",
        "attack_ms" => "Attack",
        "base_position" => "Base Pos",
        "position_step" => "Pos Step",
        "position_jitter" => "Jitter",
        "gain_decay_ratio" => "Decay",
        "min_gain_ratio" => "Gain Floor",
        "initial_gain" => "Init Gain",
        "pan_min" => "Pan Min",
        "pan_max" => "Pan Max",
        "wet" => "Wet",
        "dry" => "Dry",
        _ => id,
    };
    name.to_string()
}

/// Page tables for the side panels.
/// condition codes: 0 = always, 1 = Fixed mode, 2 = LFO mode,
/// 3 = RandomWalk mode, 4 = PeakFollow mode, 5 = spawn_sync on,
/// 6 = spawn_sync off.
type Pg = (&'static str, &'static [(&'static str, u8)]);

const LEFT_PAGES: &[Pg] = &[
    (
        "I · SPAWN",
        &[
            ("spawn_sync", 0),
            ("spawn_interval_ms", 6),
            ("spawn_interval_beats", 5),
            ("fallback_bpm", 0),
            ("max_particles", 0),
            ("reverse_chance", 0),
        ],
    ),
    (
        "II · LAW",
        &[
            ("base_position", 0),
            ("position_step", 0),
            ("position_jitter", 0),
            ("gain_decay_ratio", 0),
            ("min_gain_ratio", 0),
            ("initial_gain", 0),
        ],
    ),
    (
        "III · SHAPE",
        &[
            ("attack_ms", 0),
            ("lifetime_ms_min", 0),
            ("lifetime_ms_max", 0),
            ("freq_shift_min", 0),
            ("freq_shift_max", 0),
        ],
    ),
];

const RIGHT_PAGES: &[Pg] = &[
    (
        "I · MOVEMENT",
        &[
            ("position_mode", 0),
            ("position_smooth_ms", 0),
            ("lfo_rate_hz", 2),
            ("lfo_depth", 2),
            ("random_walk_step", 3),
            ("random_walk_interval_ms", 3),
            ("peak_window_ms", 4),
            ("peak_update_ms", 4),
            ("peak_threshold", 4),
        ],
    ),
    (
        "II · MATERIAL",
        &[
            ("texture_blend", 0),
            ("texture_window_ms", 0),
            ("texture_refresh_ms", 0),
            ("texture_stretch", 0),
            ("texture_crossfade_ms", 0),
            ("pitch_min", 0),
            ("pitch_max", 0),
        ],
    ),
    (
        "III · OUTPUT",
        &[
            ("feedback_gain", 0),
            ("feedback_delay_ms", 0),
            ("feedback_damping_hz", 0),
            ("pan_min", 0),
            ("pan_max", 0),
        ],
    ),
];

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
    /// Current page index within the panel.
    page: usize,
    /// Page-switch animation cursor: 1 right after a switch, easing to 0
    /// (drives the content crossfade).
    fade: f32,
    /// Animated panel height morphing toward the page's natural size.
    height: f32,
}

impl PanelAnim {
    fn hidden() -> Self {
        Self {
            target: false,
            opacity: 0.0,
            page: 0,
            fade: 0.0,
            height: 150.0,
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
    /// Slow whole-pattern rotation (radians).
    bg_phase: f32,
    /// Horizontal nudge of the sigil away from the visible panel (px).
    centre_shift: f32,
    /// Shuffled dot-number order: lights follow a randomized sequence across
    /// all rings instead of lighting up ring-by-ring in fixed order.
    slot_order: Vec<usize>,

    /// Panel fade animation.
    panel_left: PanelAnim,
    panel_right: PanelAnim,
    /// Whether the About overlay is open.
    about: bool,
    /// Fade cursor for the About overlay (0..1), eased every tick.
    about_fade: f32,
    /// Randomize targets being eased into (id, target) on each tick.
    randomize_pending: Vec<(String, f32)>,

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
            ParticulaMessage::ToggleLeft => {
                self.panel_left.target = !self.panel_left.target;
                self.panel_right.target = false;
            }
            ParticulaMessage::ToggleRight => {
                self.panel_right.target = !self.panel_right.target;
                self.panel_left.target = false;
            }
            ParticulaMessage::ShowAbout(show) => self.about = *show,
            ParticulaMessage::PanelPage { side, page } => {
                let anim = if *side == 0 {
                    &mut self.panel_left
                } else {
                    &mut self.panel_right
                };
                if anim.page != *page {
                    anim.page = *page;
                    anim.fade = 1.0;
                }
            }
            ParticulaMessage::Randomize => {
                self.randomize_pending = random_targets(&self.param_map);
            }
            ParticulaMessage::MasterEnabled(v) => {
                self.param_map.set("enabled", *v, std::sync::atomic::Ordering::Relaxed);
            }
            ParticulaMessage::Param { id, value } => {
                set_param_as(&self.param_map, id, *value);
                self.randomize_pending.retain(|(pid, _)| pid != id);
            }
            ParticulaMessage::ParamReset { id } => {
                if let Some((_, def)) = DEFAULTS.iter().find(|(p, _)| p == id) {
                    set_param_as(&self.param_map, id, *def);
                    self.randomize_pending.retain(|(pid, _)| pid != id);
                }
            }
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
            bg_phase: 0.0,
            centre_shift: 0.0,
            slot_order: shuffled_slots(),
            panel_left: PanelAnim::hidden(),
            panel_right: PanelAnim::hidden(),
            about: false,
            about_fade: 0.0,
            randomize_pending: Vec::new(),
            last_frame: None,
        }
    }

    /// Advance the animation by dt: ingest spawn events, fade dots, spin rings,
    /// tween panels.
    fn animate(&mut self, dt: f32) {
        // 0. Ease randomize targets into the map (no instant full-blown jump).
        if !self.randomize_pending.is_empty() {
            let mut i = 0usize;
            while i < self.randomize_pending.len() {
                let (id, target) = self.randomize_pending[i].clone();
                let cur = match self
                    .param_map
                    .get(&id)
                    .map(|av| av.load(std::sync::atomic::Ordering::Relaxed))
                {
                    Some(SetValue::Float(v)) => v,
                    Some(SetValue::Int(v)) => v as f32,
                    Some(SetValue::Bool(v)) => {
                        if v {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => target,
                };
                let next = cur + (target - cur) * 0.25;
                if (target - next).abs() < (target - cur).abs() * 0.02 + 1e-4 {
                    self.param_map.set(&id, target, std::sync::atomic::Ordering::Relaxed);
                    self.randomize_pending.remove(i);
                } else {
                    self.param_map.set(&id, next, std::sync::atomic::Ordering::Relaxed);
                    i += 1;
                }
            }
        }

        // 1. Ingest any spawn events posted by the audio thread.
        {
            let rx = self.spawn_rx.lock().expect("spawn receiver lock");
            for ev in rx.try_iter() {
                let dot_idx = self.slot_order[self.next_dot % self.slot_order.len()];
                let ring = dot_idx % RINGS;
                let slot = (dot_idx / RINGS) % DOTS_PER_RING;
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

        // 3. Rotate the rings (differential) and the whole pattern (slow).
        self.bg_phase += 0.06 * dt;
        for (r, phase) in self.ring_phases.iter_mut().enumerate() {
            *phase += RING_SPEED[r] * dt;
        }

        // 4. Tween the panels + ease page-switch cursors + morph the box
        //    height toward the current page's natural size.
        self.panel_left.update(dt);
        self.panel_right.update(dt);
        self.panel_left.fade = (self.panel_left.fade - dt * 5.5).max(0.0);
        self.panel_right.fade = (self.panel_right.fade - dt * 5.5).max(0.0);
        let k = 1.0 - (-6.0 * dt).exp();
        self.panel_left.height += (self.panel_height_target(0) - self.panel_left.height) * k;
        self.panel_right.height += (self.panel_height_target(1) - self.panel_right.height) * k;
        // About overlay fade.
        let goal = if self.about { 1.0 } else { 0.0 };
        self.about_fade += (goal - self.about_fade) * k;
        // Sigil nudges smoothly away from the visible panel.
        // Target follows the panel *intent* (target flag), so the sigil starts
        // returning the instant the panel closes; easing is faster than the
        // fade so the pattern never lags behind.
        let shift_target = if self.panel_left.target && self.panel_left.opacity > 0.004 {
            155.0
        } else if self.panel_right.target && self.panel_right.opacity > 0.004 {
            -155.0
        } else {
            0.0
        };
        let kc = 1.0 - (-6.0 * dt).exp();
        self.centre_shift += (shift_target - self.centre_shift) * kc;
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
                    .style(flat_button)
                    .padding([2, 3])
                    .width(Length::Shrink),
                text("P A R T I C U L A").font(DISPLAY).size(15).color(TEXT),
                iced::widget::space().width(Length::Fill),
                text("WET").font(MONO).size(8).color(TEXT_FAINT),
                self.mini_slider("wet"),
                text("DRY").font(MONO).size(8).color(TEXT_FAINT),
                self.mini_slider("dry"),
                self.enabled_button(),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(10),
        )
        .width(Length::Fill)
        .padding([10, 14])
        .style(panel_style(None, LINE, 0.0, 0.0));

        // ---- centre sigil: canvas + left/right click zones (half layout) ----
        let sigil = iced::widget::stack![
            iced::widget::canvas(SigilCanvas {
                dots: self.dots,
                phases: self.ring_phases,
                bg_phase: self.bg_phase,
                shift: self.centre_shift,
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
            hint_arrow(self.panel_left.opacity * 320.0, 1.0 - self.panel_left.opacity, true),
            hint_arrow(self.panel_right.opacity * 320.0, 1.0 - self.panel_right.opacity, false),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        let centre = container(sigil)
            .width(Length::Fill)
            .height(Length::Fill);

        // ---- side panel (exactly one visible at a time) ----
        // Panels float as overlay layers; the sigil stays perfectly centred
        // regardless of whether a panel fades in/out.
        let left_overlay = container(self.side_panel(0, LEFT_PAGES, self.panel_left))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Start)
            .align_y(iced::Alignment::Center);
        let right_overlay = container(self.side_panel(1, RIGHT_PAGES, self.panel_right))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::End)
            .align_y(iced::Alignment::Center);
        let body = iced::widget::stack![centre, left_overlay, right_overlay]
            .width(Length::Fill)
            .height(Length::Fill);

        // ---- footer ----
        let status_line = format!("{:<5} LIVE   {} SPAWNED   {} HZ", self.live(), self.spawned(), self.sample_rate());
        let footer = container(
            row![
                text(status_line)
                    .font(MONO)
                    .size(9)
                    .color(TEXT_FAINT),
                iced::widget::space().width(Length::Fill),
                button(text("RANDOMIZE").font(MONO).size(9).color(TEXT_DIM))
                    .on_press(ParticulaMessage::Randomize)
                    .style(flat_button),
            ]
            .align_y(iced::Alignment::Center)
            .padding([0, 14]),
        )
        .width(Length::Fill)
        .style(panel_style(Some(BG_PANEL), LINE, 1.0, 0.0));

        let base = container(
            column![header, body, footer]
                .spacing(0)
                .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([12, 18])
            .style(panel_style(Some(BG), Color::TRANSPARENT, 0.0, 0.0));

        // ---- optional About overlay ----
        if self.about_fade > 0.004 {
            let f = self.about_fade;
            let fg = Color::from_rgba(0.93, 0.93, 0.93, f);
            let dim = Color::from_rgba(0.60, 0.60, 0.60, f);
            let faint = Color::from_rgba(0.36, 0.36, 0.36, f);
            let bg_ov = Color::from_rgba(0.030, 0.030, 0.030, f * 0.92);
            let overlay: Element<'static, ParticulaMessage> = container(
                column![
                    text("PARTICULA").font(DISPLAY).size(30).color(fg),
                    text("a granular-cloud signal engine").font(MONO).size(10).color(dim),
                    text(format!("by I Am Plugins · version {}", env!("CARGO_PKG_VERSION")))
                        .font(MONO)
                        .size(9)
                        .color(faint),
                    button(text("CLOSE").font(MONO).size(9).color(fg))
                        .on_press(ParticulaMessage::ShowAbout(false))
                        .style(flat_button)
                        .padding([4, 10]),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center)
            .style(panel_style(Some(bg_ov), LINE, 1.0, 0.0))
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
        let (lo, hi, disp, log_scale) = slider_domain(id, s.min, s.max, s.value);
        iced::widget::mouse_area(
            slider(lo..=hi, disp, move |v| {
                let value = if log_scale { v.exp() } else { v };
                map.set(id, value, std::sync::atomic::Ordering::Relaxed);
                ParticulaMessage::Tick
            })
            .step(nice_step(lo, hi))
            .width(Length::Fixed(90.0))
            .style(slider_style(1.0)),
        )
        .on_double_click(ParticulaMessage::ParamReset { id })
        .into()
    }

    /// Text ON/OFF button for the master bypass.
    fn enabled_button(&self) -> Element<'static, ParticulaMessage> {
        let on = snapshot("enabled", &self.param_map)
            .map(|s| s.value > 0.5)
            .unwrap_or(true);
        let map = self.param_map.clone();
        let text_style = if on {
            TEXT
        } else {
            TEXT_FAINT
        };
        button(
            container(
                text(if on { "ON" } else { "OFF" })
                    .font(MONO)
                    .size(10)
                    .color(text_style),
            )
            .width(Length::Fill)
            .align_x(iced::Alignment::Center),
        )
        .on_press_with(move || {
            map.set("enabled", !on, std::sync::atomic::Ordering::Relaxed);
            ParticulaMessage::Tick
        })
        .style(flat_button)
        .width(Length::Fixed(56.0))
        .padding([4, 6])
        .into()
    }

    /// Natural height (px) of the current page in the given panel.
    fn panel_height_target(&self, side: usize) -> f32 {
        let pages = if side == 0 { LEFT_PAGES } else { RIGHT_PAGES };
        let page = pages[self.panel_page(side).min(pages.len().saturating_sub(1))];
        let mode = snapshot("position_mode", &self.param_map)
            .map(|s| s.value as usize)
            .unwrap_or(1);
        let sync_on = snapshot("spawn_sync", &self.param_map)
            .map(|s| s.value > 0.5)
            .unwrap_or(false);
        let rows = page
            .1
            .iter()
            .filter(|(_, cond)| cond_matches(*cond, mode, sync_on))
            .count();
        // Header bar (tabs + title) + top/bottom padding; rows at ~30 px each.
        72.0 + rows as f32 * 30.0
    }

    fn panel_page(&self, side: usize) -> usize {
        if side == 0 {
            self.panel_left.page
        } else {
            self.panel_right.page
        }
    }

    /// One fading side panel with a page picker on top and mode-filtered
    /// parameter rows (condition code matches the position_mode set).
    fn side_panel(
        &self,
        side: usize,
        pages: &'static [Pg],
        anim: PanelAnim,
    ) -> Element<'static, ParticulaMessage> {
        if anim.opacity < 0.01 {
            return iced::widget::space().width(Length::Fixed(0.0)).into();
        }
        let page = pages[anim.page.min(pages.len().saturating_sub(1))];
        let mode = snapshot("position_mode", &self.param_map)
            .map(|s| s.value as usize)
            .unwrap_or(1);
        let sync_on = snapshot("spawn_sync", &self.param_map)
            .map(|s| s.value > 0.5)
            .unwrap_or(false);
        // Page-transition cursor drives the content crossfade.
        let a = 1.0 - anim.fade * 0.72;
        let rows = page
            .1
            .iter()
            .filter(|(_, cond)| cond_matches(*cond, mode, sync_on))
            .map(|(id, _)| self.param_row(id, a))
            .collect::<Vec<_>>();

        // Title bar: page buttons inline with the title, serif: [ I ][ II ][ III ] · TITLE
        let mut title_pickers: Vec<Element<'static, ParticulaMessage>> = Vec::new();
        for (i, (ptitle, _)) in pages.iter().enumerate() {
            let active = i == anim.page;
            let num = ptitle.split(' ').next().unwrap_or("·");
            title_pickers.push(
                button(
                    text(num)
                        .font(DISPLAY)
                        .size(12)
                        .color(if active { TEXT } else { TEXT_FAINT }),
                )
                .on_press(ParticulaMessage::PanelPage { side, page: i })
                .style(page_button_style(active))
                .padding([2, 6])
                .into(),
            );
        }
        let title_bar = row![
            row(title_pickers).spacing(2),
            text("·").font(DISPLAY).size(14).color(TEXT_FAINT),
            text(page.0).font(DISPLAY).size(13).color(TEXT_DIM),
            iced::widget::space().width(Length::Fill),
        ]
        .align_y(iced::Alignment::Center)
        .spacing(8);

        let page_rows = if rows.is_empty() {
            vec![iced::widget::space().into()]
        } else {
            rows
        };

        container(
            column![
                title_bar,
                iced::widget::space().height(6),
                column(page_rows).spacing(8),
            ]
            .padding([20, 24]),
        )
        .width(Length::Fixed(anim.opacity * 320.0))
        .height(Length::Fixed(anim.height.max(20.0)))
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }

    fn param_row(&self, id: &'static str, a: f32) -> Element<'static, ParticulaMessage> {
        // Discrete (Bool / Int) parameters get option buttons instead of a
        // slider, which cannot carry their atomic value type.
        if let Some(av) = self.param_map.get(id) {
            let discrete = match &*av {
                AtomicValue::Bool { .. } => Some(["OFF", "ON"].as_slice()),
                AtomicValue::Int { .. } => Some(discrete_options(id)),
                _ => None,
            };
            if let Some(options) = discrete {
                let cur = match av.load(std::sync::atomic::Ordering::Relaxed) {
                    SetValue::Bool(v) => {
                        if v {
                            1
                        } else {
                            0
                        }
                    }
                    SetValue::Int(v) => v as usize,
                    _ => 0,
                };
                return self.discrete_row(id, options, cur, a);
            }
        }

        let Some(snap) = snapshot(id, &self.param_map) else {
            return iced::widget::space().into();
        };
        // Logarithmic scale for time/freq/stretch/pitch params: the slider
        // works on ln(value), writes are un-logged back to the real value.
        let (lo, hi, disp, log_scale) = slider_domain(id, snap.min, snap.max, snap.value);
        container(
            row![
                text(label(id))
                    .font(MONO)
                    .size(9)
                    .color(Color::from_rgba(0.60, 0.60, 0.60, a))
                    .width(Length::Fixed(76.0)),
                iced::widget::mouse_area(
                    slider(lo..=hi, disp, move |v| {
                        let value = if log_scale { v.exp() } else { v };
                        ParticulaMessage::Param { id, value }
                    })
                    .step(nice_step(lo, hi))
                    .style(slider_style(a)),
                )
                .on_double_click(ParticulaMessage::ParamReset { id }),
                text(format!("{:.2}", snap.value))
                    .font(MONO)
                    .size(9)
                    .color(Color::from_rgba(0.36, 0.36, 0.36, a))
                    .width(Length::Fixed(48.0)),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(8),
        )
        .padding([4, 14])
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }

    /// A row of option buttons for discrete (Bool / Int) parameters,
    /// styled like the page tabs (I II III).
    fn discrete_row(
        &self,
        id: &'static str,
        options: &[&'static str],
        current: usize,
        a: f32,
    ) -> Element<'static, ParticulaMessage> {
        let mut buttons: Vec<Element<'static, ParticulaMessage>> = Vec::new();
        for (i, name) in options.iter().enumerate() {
            let active = i == current;
            buttons.push(
                button(
                    container(
                        text(*name)
                            .font(MONO)
                            .size(10)
                            .color(if active { TEXT } else { TEXT_FAINT }),
                    )
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                )
                .on_press(ParticulaMessage::Param {
                    id,
                    value: i as f32,
                })
                .style(page_button_style(active))
                .padding([2, 4])
                .width(Length::FillPortion(1))
                .into(),
            );
        }
        container(
            row![
                text(label(id))
                    .font(MONO)
                    .size(9)
                    .color(Color::from_rgba(0.60, 0.60, 0.60, a))
                    .width(Length::Fixed(76.0)),
                row(buttons).spacing(4).width(Length::Fill),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(8),
        )
        .padding([4, 14])
        .style(panel_style(None, LINE, 1.0, 0.0))
        .into()
    }
}

// -------------------------------- randomize ---------------------------------
/// Factory defaults, mirroring the engine's initial parameter values.
/// (AtomicValue carries no default, so the GUI keeps its own copy.)
const DEFAULTS: &[(&str, f32)] = &[
    ("dry", 1.0),
    ("wet", 0.85),
    ("enabled", 1.0),
    ("spawn_interval_ms", 30.0),
    ("spawn_sync", 0.0),
    ("spawn_interval_beats", 0.25),
    ("fallback_bpm", 120.0),
    ("max_particles", 64.0),
    ("reverse_chance", 0.0),
    ("base_position", 0.9),
    ("position_step", 0.0),
    ("position_jitter", 0.02),
    ("gain_decay_ratio", 0.9),
    ("min_gain_ratio", 0.05),
    ("initial_gain", 0.5),
    ("attack_ms", 10.0),
    ("lifetime_ms_min", 100.0),
    ("lifetime_ms_max", 1200.0),
    ("pitch_min", 0.5),
    ("pitch_max", 1.5),
    ("freq_shift_min", -120.0),
    ("freq_shift_max", 120.0),
    ("position_smooth_ms", 20.0),
    ("position_mode", 1.0),
    ("lfo_rate_hz", 0.15),
    ("lfo_depth", 0.15),
    ("random_walk_step", 0.02),
    ("random_walk_interval_ms", 200.0),
    ("peak_window_ms", 150.0),
    ("peak_update_ms", 30.0),
    ("peak_threshold", 0.01),
    ("feedback_gain", 0.0),
    ("feedback_delay_ms", 40.0),
    ("feedback_damping_hz", 3000.0),
    ("texture_blend", 0.35),
    ("texture_window_ms", 85.0),
    ("texture_refresh_ms", 43.0),
    ("texture_stretch", 1.0),
    ("texture_crossfade_ms", 12.0),
    ("pan_min", -0.8),
    ("pan_max", 0.8),
];

/// Writes a f32 into the map with the parameter's actual atomic type
/// (Bool / Int / Float) so discrete parameters accept clicks fine.
fn set_param_as(map: &ParamMap, id: &str, v: f32) {
    use std::sync::atomic::Ordering;
    let Some(av) = map.get(id) else {
        return;
    };
    match &*av {
        AtomicValue::Bool { .. } => {
            map.set(id, v > 0.5, Ordering::Relaxed);
        }
        AtomicValue::Int { .. } => {
            map.set(id, v as i32, Ordering::Relaxed);
        }
        AtomicValue::Float { .. } => {
            map.set(id, v, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Gathers a shuffled target value for every engine parameter (except the
/// header trio) using the seeded SplitMix64 — the previous ad-hoc RNG was
/// biased toward the top of the range, so Randomize appeared to max everything
/// out. The caller eases these targets into the map over a few ticks.
fn random_targets(map: &ParamMap) -> Vec<(String, f32)> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED_FA11);
    let mut rng = SplitMix64::new(seed);
    let mut out = Vec::new();
    for i in 0..map.len() {
        let Some(id) = map.query_param_id(i).map(|s| s.to_string()) else {
            continue;
        };
        if id == "dry" || id == "wet" || id == "enabled" {
            continue;
        }
        let Some(av) = map.get_by_index(i) else {
            continue;
        };
        let value = match &*av {
            AtomicValue::Float {
                range,
                logarithmic,
                ..
            } => {
                let (lo, hi) = (*range.start(), *range.end());
                if *logarithmic && lo > 0.0 {
                    lo * (hi / lo).powf(rng.next_f32())
                } else {
                    rng.range(lo, hi)
                }
            }
            AtomicValue::Int { range, .. } => {
                let (lo, hi) = (*range.start(), *range.end());
                let span = (hi - lo) as u32 + 1;
                (lo + (rng.next_f32() * span as f32) as i32) as f32
            }
            AtomicValue::Bool { .. } => {
                if rng.next_f32() > 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => continue,
        };
        out.push((id, value));
    }
    out
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
        },
        ..container::Style::default()
    }
}

fn page_button_style(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style + 'static {
    move |_: &iced::Theme, status: button::Status| {
        let border = if active || matches!(status, button::Status::Hovered | button::Status::Pressed) {
            LINE
        } else {
            Color::TRANSPARENT
        };
        button::Style {
            background: None,
            text_color: TEXT,
            border: iced::Border {
                color: border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

/// A faint chevron hinting the clickable half (fades out while the panel
/// on that side is visible). Drawn as a canvas triangle, not an emoji.
fn hint_arrow(edge: f32, alpha: f32, left: bool) -> Element<'static, ParticulaMessage> {
    iced::widget::canvas(HintChevron {
        left,
        a: alpha.clamp(0.0, 1.0) * 0.6,
        edge,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct HintChevron {
    left: bool,
    a: f32,
    /// Panel expansion width; the chevron tracks the panel's leading edge so
    /// it moves with the panel itself (0..320), not just the sigil nudge.
    edge: f32,
}

impl<M> canvas::Program<M> for HintChevron {
    type State = ();
    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use iced::widget::canvas::Path;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let cy = bounds.height * 0.5;
        // Chevron rides the panel's leading edge (inwards from the screen edge).
        let tip_x = if self.left {
            26.0 + self.edge
        } else {
            bounds.width - 26.0 - self.edge
        };
        let dir: f32 = if self.left { 1.0 } else { -1.0 };
        let s = 8.0;
        let tri = Path::new(|builder| {
            builder.move_to(iced::Point::new(tip_x, cy));
            builder.line_to(iced::Point::new(tip_x - dir * s, cy - s * 0.8));
            builder.line_to(iced::Point::new(tip_x - dir * s, cy + s * 0.8));
            builder.close();
        });
        frame.fill(&tri, Color::from_rgba(1.0, 1.0, 1.0, self.a));
        vec![frame.into_geometry()]
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
        },
        ..Default::default()
    }
}

fn slider_style(a: f32) -> impl Fn(&iced::Theme, slider::Status) -> slider::Style + 'static {
    move |_: &iced::Theme, _: slider::Status| {
        use iced::widget::slider::{Handle, HandleShape, Rail};
        slider::Style {
            rail: Rail {
                backgrounds: (
                    iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.28 * a)),
                    iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.10 * a)),
                ),
                width: 2.0,
                border: iced::Border::default(),
            },
            handle: Handle {
                shape: HandleShape::Circle { radius: 5.0 },
                background: iced::Background::Color(Color::from_rgba(0.93, 0.93, 0.93, a)),
                border_width: 0.0,
                border_color: iced::Color::TRANSPARENT,
            },
        }
    }
}

// -------------------------------- the sigil ---------------------------------
/// Slot numbers 0..RINGS*DOTS_PER_RING in a fixed, shuffled order (lighting
/// appears scattered across every ring rather than sequential).
fn shuffled_slots() -> Vec<usize> {
    let mut order: Vec<usize> = (0..RINGS * DOTS_PER_RING).collect();
    let mut rng = SplitMix64::new(0x51A6);
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// Vertices of a regular polygon around `center`.
fn polygon_points(
    n: usize,
    radius: f32,
    phase: f32,
    center: iced::Point,
) -> Vec<iced::Point> {
    (0..n)
        .map(|i| {
            let a = phase + i as f32 / n as f32 * std::f32::consts::TAU;
            iced::Point::new(center.x + a.cos() * radius, center.y + a.sin() * radius)
        })
        .collect()
}

/// The living sigil: sacred-geometry lines, particle orbits and lit dots.
struct SigilCanvas {
    dots: [RingSlots; RINGS],
    phases: [f32; RINGS],
    /// Slow rotation shared by the whole background pattern (radians).
    bg_phase: f32,
    /// Horizontal nudge applied to the pattern centre (px).
    shift: f32,
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
        let centre = frame.center();
        let c = iced::Point::new(centre.x + self.shift, centre.y);
        let max_r = w.min(h) * 0.5;

        use iced::widget::canvas::{Path, Stroke};
        let hairline = |a: f32| Stroke {
            width: 1.0,
            style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, a)),
            ..Stroke::default()
        };

        // Sacred geometry (homology floral rose): flower of six circles,
        // inscribed diamond, double triangle (hexagram), node dots. Drawn with
        // straight lines / hairlines only; the whole pattern breathes at a
        // slow rate.
        let bg = self.bg_phase;

        // Flower: six circles orbit around the centre.
        let flower_center_r = max_r * 0.46;
        let petal_r = max_r * 0.34;
        for i in 0..6_usize {
            let a = bg + i as f32 / 6.0 * std::f32::consts::TAU;
            let pc = iced::Point::new(c.x + a.cos() * flower_center_r, c.y + a.sin() * flower_center_r);
            frame.stroke(&Path::circle(pc, petal_r), hairline(0.05));
        }

        // Diamond (inscribed square, rotated).
        let quad = polygon_points(4, max_r * 0.62, bg + 0.25 * std::f32::consts::PI, c);
        for i in 0..4_usize {
            frame.stroke(&Path::line(quad[i], quad[(i + 1) % 4]), hairline(0.12));
        }

        // Double triangle (hexagram).
        let tri_a = polygon_points(3, max_r * 0.48, bg + std::f32::consts::PI / 6.0, c);
        let tri_b = polygon_points(3, max_r * 0.48, bg + std::f32::consts::PI / 6.0 + std::f32::consts::PI, c);
        for tri in [&tri_a, &tri_b] {
            for i in 0..3_usize {
                frame.stroke(&Path::line(tri[i], tri[(i + 1) % 3]), hairline(0.16));
            }
        }

        // Node dots at the vertices.
        for p in quad.iter().chain(tri_a.iter()).chain(tri_b.iter()) {
            frame.fill(&Path::circle(*p, 1.2), Color::from_rgba(1.0, 1.0, 1.0, 0.22));
        }

        // Three particle orbits, differential rotation.
        for (ring, &phase) in self.phases.iter().enumerate() {
            let r = max_r * (0.80 - ring as f32 * 0.16);
            frame.stroke(&Path::circle(c, r), hairline(0.12));

            // Dots: faint skeleton + lit spawns.
            for slot in 0..DOTS_PER_RING {
                let base_angle = slot as f32 / DOTS_PER_RING as f32 * std::f32::consts::TAU;
                let angle = base_angle + phase;
                let dot_pos = iced::Point::new(c.x + angle.cos() * r, c.y + angle.sin() * r);
                let d = &self.dots[ring][slot];
                let alpha = dot_alpha(d);
                if alpha > 0.02 {
                    frame.fill(&Path::circle(dot_pos, 2.6), Color::from_rgba(1.0, 1.0, 1.0, alpha));
                } else {
                    frame.fill(&Path::circle(dot_pos, 1.2), Color::from_rgba(1.0, 1.0, 1.0, 0.05));
                }
            }
        }

        // Centre: small ring + core dot.
        frame.stroke(&Path::circle(c, max_r * 0.07), hairline(0.35));
        frame.fill(&Path::circle(c, 2.0), Color::from_rgba(1.0, 1.0, 1.0, 0.9));

        // Split indicator (faint vertical divider between the click zones).
        frame.stroke(
            &Path::line(
                iced::Point::new(c.x, c.y - max_r * 0.4),
                iced::Point::new(c.x, c.y + max_r * 0.4),
            ),
            hairline(0.06),
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
