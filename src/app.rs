//! egui/eframe user interface for Autoclicker 2.0.
//!
//! The layout follows the familiar sections of OP Auto Clicker (interval,
//! mouse button, click type, repeat, cursor position) and adds assignable
//! hotkeys, a key-bound macro and a live measured CPS readout.
//!
//! The UI only displays state and forwards settings to the engine: hotkeys
//! (`hotkeys.rs`), the click loop (`engine.rs`) and the macro
//! (`macro_engine.rs`) run on their own threads, so everything keeps working
//! while the window is minimized or while rendering is paused.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui::{Color32, CornerRadius, Margin, RichText, Stroke};

use crate::config::Settings;
use crate::engine::{self, STOP_CLICK_LIMIT, STOP_TIME_LIMIT, Shared};
use crate::hotkeys;
use crate::macro_engine;
use crate::sys;

pub const APP_TITLE: &str = "Autoclicker 2.0";
/// Fixed window size (logical points). Full screen is not supported by design.
pub const WINDOW_W: f32 = 520.0;
pub const WINDOW_H: f32 = 730.0;

// ── Colour palette (dark amber theme) ─────────────────────────────────────
const ACCENT: Color32 = Color32::from_rgb(255, 138, 66);
const ACCENT_DIM: Color32 = Color32::from_rgb(72, 42, 22);
const BG: Color32 = Color32::from_rgb(17, 18, 21);
const SURFACE: Color32 = Color32::from_rgb(24, 26, 31);
const SURFACE_INPUT: Color32 = Color32::from_rgb(16, 17, 20);
const SURFACE_HOVER: Color32 = Color32::from_rgb(34, 37, 43);
const BORDER: Color32 = Color32::from_rgb(46, 50, 57);
const TEXT: Color32 = Color32::from_rgb(233, 236, 241);
const TEXT_DIM: Color32 = Color32::from_rgb(146, 153, 163);
const GREEN: Color32 = Color32::from_rgb(63, 185, 80);
const AMBER: Color32 = Color32::from_rgb(232, 178, 58);
const RED: Color32 = Color32::from_rgb(229, 72, 77);
const GREEN_FILL: Color32 = Color32::from_rgb(28, 84, 44);
const RED_FILL: Color32 = Color32::from_rgb(74, 26, 29);
const RED_TEXT: Color32 = Color32::from_rgb(255, 138, 138);

// Assignment slots (same numbers as `hotkeys.rs`).
const SLOT_TOGGLE: u8 = 1;
const SLOT_STOP: u8 = 2;
const SLOT_PICK: u8 = 3;
const SLOT_MACRO: u8 = 4;

/// Smooth transition duration for the main toggle and status badge.
const ANIM_FAST: f32 = 0.28;

fn install_theme(ctx: &egui::Context) {
    // eframe 0.37 has no `Context::style()/set_style()`; override every style
    // variant (dark/light) with the same palette in one pass.
    ctx.all_styles_mut(|style| {
        let mut v = egui::Visuals::dark();

        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = SURFACE_INPUT;
        v.faint_bg_color = Color32::from_rgb(30, 33, 38);
        v.override_text_color = Some(TEXT);

        v.widgets.noninteractive.bg_fill = SURFACE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);

        v.widgets.inactive.bg_fill = SURFACE_HOVER;
        v.widgets.inactive.weak_bg_fill = SURFACE_HOVER;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);

        v.widgets.hovered.bg_fill = Color32::from_rgb(43, 47, 54);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(43, 47, 54);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.75));
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);

        v.widgets.active.bg_fill = ACCENT_DIM;
        v.widgets.active.weak_bg_fill = ACCENT_DIM;
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(8);
        }

        v.selection.bg_fill = ACCENT.gamma_multiply(0.38);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.slider_trailing_fill = true;

        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(11.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.scroll.bar_width = 8.0;
        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::proportional(20.0)),
            (egui::TextStyle::Body, egui::FontId::proportional(14.0)),
            (egui::TextStyle::Button, egui::FontId::proportional(14.0)),
            (egui::TextStyle::Small, egui::FontId::proportional(11.5)),
            (egui::TextStyle::Monospace, egui::FontId::monospace(13.0)),
        ]
        .into();
    });
}

pub struct AutoclickerApp {
    shared: Arc<Shared>,
    settings: Settings,
    /// Settings last written to disk — used for the "did it change?" check.
    saved: Settings,
    /// Latest change seen and when it happened (debounce).
    pending: Option<Settings>,
    last_change: Instant,

    // Live statistics
    total_clicks: u64,
    session_clicks: u64,
    live_cps: f32,
    last_sample_clicks: u64,
    last_sample: Instant,

    // Position-capture feedback
    last_capture_pulse: u64,
    flash_until: Option<Instant>,

    // Hotkey assignment
    show_hotkeys: bool,
    pending_slot: Option<u8>,
    hotkey_error: Option<String>,

    // Window level (always on top) is only sent when it changes.
    applied_top: Option<bool>,
}

impl AutoclickerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);

        let settings = Settings::load();
        let shared = Arc::new(Shared::from_settings(&settings));
        engine::spawn(Arc::clone(&shared));
        hotkeys::spawn(Arc::clone(&shared));
        macro_engine::spawn(Arc::clone(&shared));

        Self {
            shared,
            saved: settings.clone(),
            settings,
            pending: None,
            last_change: Instant::now(),
            total_clicks: 0,
            session_clicks: 0,
            live_cps: 0.0,
            last_sample_clicks: 0,
            last_sample: Instant::now(),
            last_capture_pulse: 0,
            flash_until: None,
            show_hotkeys: false,
            pending_slot: None,
            hotkey_error: None,
            applied_top: None,
        }
    }

    /// UI → engine transfer (every frame). A handful of `Relaxed` stores cost
    /// practically nothing, so no "did it change?" tracking is needed.
    fn push_settings(&self) {
        let s = &self.settings;
        self.shared.interval_us.store(s.interval_us(), Relaxed);
        self.shared.jitter_us.store(
            if s.jitter_enabled {
                (s.jitter_ms * 1_000.0) as u32
            } else {
                0
            },
            Relaxed,
        );
        self.shared.button.store(s.button, Relaxed);
        self.shared.double_click.store(s.double_click, Relaxed);
        self.shared.start_delay_ms.store(s.start_delay_ms, Relaxed);
        self.shared
            .limit_clicks
            .store(if s.repeat_forever { 0 } else { s.repeat_count }, Relaxed);
        self.shared.limit_secs.store(s.limit_secs, Relaxed);
        self.shared.fixed_pos.store(s.fixed_pos, Relaxed);
        self.shared.pos_x.store(s.pos_x, Relaxed);
        self.shared.pos_y.store(s.pos_y, Relaxed);

        self.shared.hotkey_toggle.store(s.hotkey_toggle, Relaxed);
        self.shared.hotkey_stop.store(s.hotkey_stop, Relaxed);
        self.shared.hotkey_pick.store(s.hotkey_pick, Relaxed);
        self.shared.hotkey_macro.store(s.hotkey_macro, Relaxed);

        self.shared.macro_button.store(s.macro_button, Relaxed);
        self.shared.macro_clicks.store(s.macro_clicks, Relaxed);
        self.shared
            .macro_interval_us
            .store(s.macro_interval_ms * 1_000, Relaxed);
    }

    /// Live statistics from the engine's atomics. The measured rate is the
    /// number of clicks in a 0.5 s window (actual, not target).
    fn update_stats(&mut self) {
        let total = self.shared.total_clicks.load(Relaxed);
        self.session_clicks = self.shared.session_clicks.load(Relaxed);
        self.total_clicks = total;

        let dt = self.last_sample.elapsed().as_secs_f32();
        if dt >= 0.5 {
            self.live_cps = total.saturating_sub(self.last_sample_clicks) as f32 / dt;
            self.last_sample_clicks = total;
            self.last_sample = Instant::now();
        }

        let pulse = self.shared.capture_pulse.load(Relaxed);
        if pulse != self.last_capture_pulse {
            self.last_capture_pulse = pulse;
            self.flash_until = Some(Instant::now() + Duration::from_millis(1_200));
            self.settings.fixed_pos = true;
        }
    }

    fn save_if_changed(&mut self) {
        if self.settings == self.saved {
            self.pending = None;
            return;
        }
        // Debounce: dragging a slider must not write to disk on every pixel.
        // One write happens 700 ms after the last change.
        match &self.pending {
            Some(p) if *p == self.settings => {
                if self.last_change.elapsed() >= Duration::from_millis(700) {
                    self.settings = self.settings.clone().sanitize();
                    self.settings.save();
                    self.saved = self.settings.clone();
                    self.pending = None;
                }
            }
            _ => {
                self.pending = Some(self.settings.clone());
                self.last_change = Instant::now();
            }
        }
    }

    /// Reads the capture result from the engine and stores it in the slot.
    fn poll_capture(&mut self) {
        if self.shared.capture_slot.load(Relaxed) != 0 {
            return;
        }
        let captured = self.shared.captured_vk.swap(0, Relaxed);
        if captured == 0 {
            return;
        }
        let Some(slot) = self.pending_slot.take() else {
            return;
        };
        if captured < 0 {
            return; // cancelled with Esc
        }
        let vk = captured as u32;
        if self.hotkey_used_by_other(slot, vk) {
            self.hotkey_error = Some(format!(
                "{} is already assigned to another action. Change that action first.",
                key_name(vk)
            ));
            return;
        }
        self.hotkey_error = None;
        match slot {
            SLOT_TOGGLE => self.settings.hotkey_toggle = vk,
            SLOT_STOP => self.settings.hotkey_stop = vk,
            SLOT_PICK => self.settings.hotkey_pick = vk,
            SLOT_MACRO => self.settings.hotkey_macro = vk,
            _ => {}
        }
    }

    fn hotkey_used_by_other(&self, slot: u8, vk: u32) -> bool {
        let s = &self.settings;
        [
            (SLOT_TOGGLE, s.hotkey_toggle),
            (SLOT_STOP, s.hotkey_stop),
            (SLOT_PICK, s.hotkey_pick),
            (SLOT_MACRO, s.hotkey_macro),
        ]
        .iter()
        .any(|(other, key)| *other != slot && *key == vk)
    }

    fn begin_capture(&mut self, slot: u8) {
        self.pending_slot = Some(slot);
        self.hotkey_error = None;
        self.shared.captured_vk.store(0, Relaxed);
        self.shared.capture_slot.store(slot, Relaxed);
    }

    fn stop_reason(&self) -> Option<&'static str> {
        match self.shared.stop_reason.load(Relaxed) {
            STOP_CLICK_LIMIT => Some("Click limit reached — stopped automatically."),
            STOP_TIME_LIMIT => Some("Time limit reached — stopped automatically."),
            _ => None,
        }
    }
}

impl Drop for AutoclickerApp {
    /// Whatever path closes the window (X button, OS shutdown), stop the
    /// engine and persist the latest settings.
    fn drop(&mut self) {
        self.shared.stop();
        self.shared.macro_cancel.store(true, Relaxed);
        self.shared.shutdown.store(true, Relaxed);
        // Never leave a mouse button held when the window closes.
        sys::release_all();
        if self.settings != self.saved {
            self.settings.save();
        }
    }
}

impl eframe::App for AutoclickerApp {
    /// Called before `ui` on every frame.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.push_settings();
        self.update_stats();
        self.poll_capture();
        self.save_if_changed();

        if self.applied_top != Some(self.settings.always_on_top) {
            let level = if self.settings.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            self.applied_top = Some(self.settings.always_on_top);
        }

        // 30 ms keeps counters, countdown and macro progress smooth.
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(Margin::same(8)).fill(BG))
            .show(ui, |ui| {
                // The window is fixed-size; the content can grow taller than
                // the viewport (e.g. both cards expanded), so everything is
                // inside a vertical scroll area with a slim scrollbar.
                egui::ScrollArea::vertical()
                    .id_salt("main_scroll")
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .show(ui, |ui| {
                        self.header(ui);
                        self.interval_card(ui);
                        self.click_card(ui);
                        self.position_card(ui);
                        self.start_stop(ui);
                        self.macro_card(ui);
                        self.advanced(ui);
                        self.footer(ui);
                        ui.add_space(6.0);
                    });
            });

        let ctx = ui.ctx().clone();
        self.hotkeys_window(&ctx);
    }
}

impl AutoclickerApp {
    // ── Top bar ───────────────────────────────────────────────────────────
    fn header(&mut self, ui: &mut egui::Ui) {
        let running = self.shared.is_running();
        let countdown = self.shared.countdown_ms.load(Relaxed);
        let t =
            ui.ctx()
                .animate_bool_with_time(egui::Id::unique("status-pill"), running, ANIM_FAST);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Autoclicker").size(21.0).strong().color(TEXT));
                    ui.label(RichText::new("2.0").size(11.0).strong().color(ACCENT));
                    // Status badge on the right of the title row: keeps the
                    // second line free for live statistics.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if !running {
                            "READY"
                        } else if countdown > 0 {
                            "STARTING"
                        } else {
                            "RUNNING"
                        };
                        // Amber while counting down; otherwise a smooth
                        // dim→green cross-fade driven by the same animation.
                        let color = if running && countdown > 0 {
                            AMBER
                        } else {
                            mix(TEXT_DIM, GREEN, t)
                        };
                        pill(ui, label, color);
                    });
                });
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(
                            RichText::new(format!("{:.1}", self.live_cps))
                                .size(11.0)
                                .strong()
                                .color(if running { GREEN } else { TEXT_DIM }),
                        )
                        .on_hover_text("Measured clicks per second (actual, not target)");
                        ui.label(RichText::new("cps").size(9.0).color(TEXT_DIM));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(self.session_clicks.to_string())
                                .size(11.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(RichText::new("session").size(9.0).color(TEXT_DIM));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(self.total_clicks.to_string())
                                .size(11.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(RichText::new("total").size(9.0).color(TEXT_DIM));
                    });
                });
            }
        });

        if running && countdown > 0 {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!(
                    "Starting in {:.1}s — switch to the target window.",
                    countdown as f32 / 1000.0
                ))
                .size(11.0)
                .color(AMBER),
            );
        }
    }

    // ── Click interval ────────────────────────────────────────────────────
    fn interval_card(&mut self, ui: &mut egui::Ui) {
        card(ui, "CLICK INTERVAL", |ui| {
            ui.horizontal(|ui| {
                unit_field(ui, &mut self.settings.interval_h, 23, "hours");
                unit_field(ui, &mut self.settings.interval_m, 59, "min");
                unit_field(ui, &mut self.settings.interval_s, 59, "sec");
                unit_field(ui, &mut self.settings.interval_ms, 999, "ms");
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("≈ {:.1} clicks/sec", self.settings.cps()))
                        .size(12.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(self.settings.jitter_enabled, |ui| {
                        ui.add_sized(
                            [70.0, 24.0],
                            egui::DragValue::new(&mut self.settings.jitter_ms)
                                .range(0.0..=200.0)
                                .speed(0.5)
                                .suffix(" ms"),
                        );
                    });
                    ui.label(RichText::new("±").size(12.0).color(TEXT_DIM));
                    ui.checkbox(
                        &mut self.settings.jitter_enabled,
                        RichText::new("Random offset").size(12.0),
                    )
                    .on_hover_text("Triangular timing jitter with occasional human hesitations");
                });
            });
        });
    }

    // ── Click type + repeat ───────────────────────────────────────────────
    fn click_card(&mut self, ui: &mut egui::Ui) {
        card(ui, "CLICK", |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Mouse button").size(11.0).color(TEXT_DIM));
                    segmented(
                        ui,
                        &mut self.settings.button,
                        &[(0, "Left"), (1, "Right"), (2, "Middle")],
                    );
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Click type").size(11.0).color(TEXT_DIM));
                    let mut kind = u8::from(self.settings.double_click);
                    segmented(ui, &mut kind, &[(0, "Single"), (1, "Double")]);
                    self.settings.double_click = kind == 1;
                });
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut self.settings.repeat_forever,
                    true,
                    "Repeat until stopped",
                );
                ui.radio_value(&mut self.settings.repeat_forever, false, "Stop after");
                if !self.settings.repeat_forever {
                    ui.add_sized(
                        [86.0, 26.0],
                        egui::DragValue::new(&mut self.settings.repeat_count)
                            .range(1..=1_000_000)
                            .speed(1.0),
                    );
                    ui.label(RichText::new("clicks").size(12.0).color(TEXT_DIM));
                }
            });
        });
    }

    // ── Cursor position ───────────────────────────────────────────────────
    fn position_card(&mut self, ui: &mut egui::Ui) {
        let pick_key = key_name(self.settings.hotkey_pick);

        card(ui, "CURSOR POSITION", |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.settings.fixed_pos, false, "Follow cursor");
                ui.radio_value(&mut self.settings.fixed_pos, true, "Fixed position");
            });

            ui.horizontal(|ui| {
                let mut capture = false;
                ui.add_enabled_ui(self.settings.fixed_pos, |ui| {
                    ui.label(RichText::new("X").size(11.0).color(TEXT_DIM));
                    ui.add_sized(
                        [78.0, 26.0],
                        egui::DragValue::new(&mut self.settings.pos_x)
                            .range(-20_000..=20_000)
                            .speed(1.0),
                    );
                    ui.label(RichText::new("Y").size(11.0).color(TEXT_DIM));
                    ui.add_sized(
                        [78.0, 26.0],
                        egui::DragValue::new(&mut self.settings.pos_y)
                            .range(-20_000..=20_000)
                            .speed(1.0),
                    );
                    capture = ui
                        .button(RichText::new(format!("⌖  Pick ({pick_key})")).size(12.0))
                        .on_hover_text("Save the cursor's current position")
                        .clicked();
                });
                if capture {
                    if let Some((x, y)) = sys::cursor_pos() {
                        self.settings.pos_x = x;
                        self.settings.pos_y = y;
                        self.settings.fixed_pos = true;
                        self.flash_until = Some(Instant::now() + Duration::from_millis(1_200));
                    }
                }
            });

            if self.flash_until.is_some_and(|t| Instant::now() < t) {
                ui.label(
                    RichText::new("✔ Position saved — clicks will go to this point")
                        .size(11.0)
                        .strong()
                        .color(GREEN),
                );
            }
        });
    }

    // ── Start / stop ──────────────────────────────────────────────────────
    fn start_stop(&mut self, ui: &mut egui::Ui) {
        let running = self.shared.is_running();
        let toggle_key = key_name(self.settings.hotkey_toggle);

        // The whole button cross-fades between green and red instead of
        // snapping — and since this is driven by `animate_bool_with_time`,
        // both the colour and the repaint scheduling are handled by egui.
        let t =
            ui.ctx()
                .animate_bool_with_time(egui::Id::unique("start-stop-btn"), running, ANIM_FAST);
        let fill = mix(GREEN_FILL, RED_FILL, t);
        let edge = mix(GREEN, RED, t);
        let fg = mix(Color32::WHITE, RED_TEXT, t);
        let (icon, verb) = if running {
            ("■", "STOP")
        } else {
            ("▶", "START")
        };
        let text = format!("{icon}   {verb}   ({toggle_key})");

        let btn = egui::Button::new(RichText::new(text).size(16.0).strong().color(fg))
            .fill(fill)
            .stroke(Stroke::new(1.0, edge.gamma_multiply(0.8)))
            .corner_radius(CornerRadius::same(12))
            .min_size(egui::vec2(ui.available_width(), 48.0));

        if ui.add(btn).clicked() {
            self.shared.toggle();
            self.last_sample_clicks = self.shared.total_clicks.load(Relaxed);
            self.last_sample = Instant::now();
        }

        // If a repeat limit is set, show progress; if it auto-stopped, say why.
        if !self.settings.repeat_forever {
            let target = self.settings.repeat_count.max(1);
            let done = (self.session_clicks.min(target as u64) as f32) / target as f32;
            ui.add(
                egui::ProgressBar::new(done)
                    .desired_height(10.0)
                    .fill(ACCENT)
                    .text(
                        RichText::new(format!(
                            "{} / {} clicks",
                            self.session_clicks.min(target as u64),
                            target
                        ))
                        .size(10.5),
                    ),
            );
        }
        if let Some(reason) = self.stop_reason() {
            ui.label(RichText::new(reason).size(11.0).color(AMBER));
        }
    }

    // ── Macro ─────────────────────────────────────────────────────────────
    fn macro_card(&mut self, ui: &mut egui::Ui) {
        let key = key_name(self.settings.hotkey_macro);
        let stop_key = key_name(self.settings.hotkey_stop);
        let active = self.shared.macro_active.load(Relaxed);
        let progress = self.shared.macro_progress.load(Relaxed);

        let hint = format!("Press {key} for a one-shot burst. {stop_key} cancels it.");

        card_with_hint(ui, "MACRO", &hint, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Button").size(11.0).color(TEXT_DIM));
                    segmented(
                        ui,
                        &mut self.settings.macro_button,
                        &[(0, "Left"), (1, "Right")],
                    );
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Clicks").size(11.0).color(TEXT_DIM));
                    ui.add_sized(
                        [84.0, 26.0],
                        egui::DragValue::new(&mut self.settings.macro_clicks)
                            .range(1..=100_000)
                            .speed(1.0),
                    );
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Interval").size(11.0).color(TEXT_DIM));
                    ui.add_sized(
                        [84.0, 26.0],
                        egui::DragValue::new(&mut self.settings.macro_interval_ms)
                            .range(1..=10_000)
                            .speed(1.0)
                            .suffix(" ms"),
                    );
                });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let run = egui::Button::new(
                    RichText::new(format!("▶  Run macro  ({key})"))
                        .size(12.5)
                        .strong(),
                )
                .fill(ACCENT_DIM)
                .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.7)))
                .corner_radius(CornerRadius::same(8));
                if ui.add(run).clicked() {
                    self.shared.macro_request.store(true, Relaxed);
                }

                let cancel =
                    egui::Button::new(RichText::new(format!("Stop  ({stop_key})")).size(12.0))
                        .corner_radius(CornerRadius::same(8));
                if ui.add(cancel).clicked() {
                    self.shared.macro_cancel.store(true, Relaxed);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let total = self.settings.macro_clicks.max(1);
                    let text = if active {
                        format!("{progress} / {total}")
                    } else {
                        format!("ready · {total} clicks")
                    };
                    ui.label(RichText::new(text).size(11.0).color(if active {
                        ACCENT
                    } else {
                        TEXT_DIM
                    }));
                });
            });

            if active {
                let total = self.settings.macro_clicks.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new((progress as f32 / total).clamp(0.0, 1.0))
                        .desired_height(6.0)
                        .fill(ACCENT)
                        .animate(true),
                );
            }
        });
    }

    // ── Advanced (starts collapsed) ───────────────────────────────────────
    fn advanced(&mut self, ui: &mut egui::Ui) {
        // `CollapsingHeader` animates its own open/close transition.
        egui::CollapsingHeader::new(RichText::new("Advanced").size(12.0).color(TEXT_DIM))
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Start delay").size(12.0).color(TEXT_DIM));
                    let mut secs = self.settings.start_delay_ms / 1000;
                    if ui
                        .add_sized(
                            [70.0, 26.0],
                            egui::DragValue::new(&mut secs).range(0..=10).suffix(" s"),
                        )
                        .changed()
                    {
                        self.settings.start_delay_ms = secs * 1000;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Time limit").size(12.0).color(TEXT_DIM));
                    ui.add_sized(
                        [70.0, 26.0],
                        egui::DragValue::new(&mut self.settings.limit_secs)
                            .range(0..=3600)
                            .suffix(" s"),
                    );
                    ui.label(RichText::new("(0 = none)").size(10.5).color(TEXT_DIM));
                });
                ui.checkbox(
                    &mut self.settings.always_on_top,
                    RichText::new("Always on top").size(12.5),
                );
                if ui
                    .button(RichText::new("Reset counters").size(12.0))
                    .clicked()
                {
                    self.shared.total_clicks.store(0, Relaxed);
                    self.shared.session_clicks.store(0, Relaxed);
                    self.total_clicks = 0;
                    self.session_clicks = 0;
                    self.last_sample_clicks = 0;
                    self.last_sample = Instant::now();
                    self.live_cps = 0.0;
                }
            });
    }

    // ── Bottom bar ────────────────────────────────────────────────────────
    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("⌨  Hotkeys").size(12.5)).clicked() {
                self.show_hotkeys = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.hyperlink_to(
                    RichText::new("github.com/lazecore  ↗")
                        .size(11.0)
                        .color(ACCENT),
                    "https://github.com/lazecore",
                )
                .on_hover_text("Author profile on GitHub");
            });
        });
    }

    // ── Hotkey assignment window ──────────────────────────────────────────
    fn hotkeys_window(&mut self, ctx: &egui::Context) {
        // Fade the dialog in and out instead of popping it.
        let fade =
            ctx.animate_bool_with_time(egui::Id::unique("hotkeys-window"), self.show_hotkeys, 0.18);
        if !self.show_hotkeys && fade < 0.01 {
            return;
        }

        let mut open = self.show_hotkeys;
        egui::Window::new(RichText::new("Hotkeys").size(15.0).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                ui.set_width(340.0);
                ui.label(
                    RichText::new("Click Change, then press the key you want. Esc cancels.")
                        .size(11.0)
                        .color(TEXT_DIM),
                );
                ui.add_space(8.0);

                let rows = [
                    (SLOT_TOGGLE, "Start / Stop", self.settings.hotkey_toggle),
                    (SLOT_STOP, "Stop (incl. macro)", self.settings.hotkey_stop),
                    (SLOT_PICK, "Pick position", self.settings.hotkey_pick),
                    (SLOT_MACRO, "Macro", self.settings.hotkey_macro),
                ];

                for (slot, label, vk) in rows {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(12.5));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let waiting = self.pending_slot == Some(slot)
                                    && self.shared.capture_slot.load(Relaxed) != 0;
                                if waiting {
                                    ui.label(
                                        RichText::new("press a key…")
                                            .size(11.0)
                                            .strong()
                                            .color(AMBER),
                                    );
                                } else {
                                    key_chip(ui, &key_name(vk));
                                    if ui
                                        .button(RichText::new("Change").size(11.5))
                                        .clicked()
                                    {
                                        self.begin_capture(slot);
                                    }
                                }
                            },
                        );
                    });
                }

                if let Some(err) = &self.hotkey_error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(err).size(11.0).color(RED));
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "A key can only be assigned once. Hotkeys keep working while this window is minimized.",
                    )
                    .size(10.5)
                    .color(TEXT_DIM),
                );
            });
        if !open {
            self.show_hotkeys = false;
        }
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────

/// Section card: title + content.
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_with_hint(ui, title, "", add);
}

/// Section card with an optional one-line hint under the title.
fn card_with_hint(ui: &mut egui::Ui, title: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(10.5).strong().color(ACCENT));
            if !hint.is_empty() {
                ui.add_space(1.0);
                ui.label(RichText::new(hint).size(10.5).color(TEXT_DIM));
            }
            ui.add_space(5.0);
            add(ui);
        });
}

/// Segmented selection buttons (Left / Right / Middle, etc.).
fn segmented(ui: &mut egui::Ui, value: &mut u8, options: &[(u8, &str)]) {
    egui::Frame::NONE
        .fill(SURFACE_INPUT)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (val, label) in options {
                    let selected = *value == *val;
                    let btn = egui::Button::new(
                        RichText::new(*label)
                            .size(12.5)
                            .strong()
                            .color(if selected { Color32::WHITE } else { TEXT_DIM }),
                    )
                    .fill(if selected {
                        ACCENT_DIM
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(if selected {
                        Stroke::new(1.0, ACCENT.gamma_multiply(0.8))
                    } else {
                        Stroke::NONE
                    })
                    .corner_radius(CornerRadius::same(7))
                    .min_size(egui::vec2(44.0, 24.0));
                    if ui.add(btn).clicked() {
                        *value = *val;
                    }
                }
            });
        });
}

/// Interval component (value with its unit name underneath).
fn unit_field(ui: &mut egui::Ui, value: &mut u32, max: u32, unit: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_sized(
            [64.0, 28.0],
            egui::DragValue::new(value).range(0..=max).speed(0.25),
        );
        ui.label(RichText::new(unit).size(9.5).color(TEXT_DIM));
    });
}

/// Status badge.
fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(30, 33, 38))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.label(RichText::new("●").size(9.0).color(color));
                ui.label(RichText::new(text).size(10.5).strong().color(color));
            });
        });
}

/// Key-name badge.
fn key_chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(SURFACE_INPUT)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .monospace()
                    .size(11.5)
                    .strong()
                    .color(ACCENT),
            );
        });
}

/// Linear interpolation between two colours (for smooth state transitions).
fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(from.r(), to.r()),
        lerp(from.g(), to.g()),
        lerp(from.b(), to.b()),
        lerp(from.a(), to.a()),
    )
}

/// Translates a Windows virtual-key code into a readable name.
pub fn key_name(vk: u32) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x13 => "Pause".into(),
        0x14 => "CapsLock".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x2C => "PrtSc".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x30..=0x39 => char::from(vk as u8).to_string(),
        0x41..=0x5A => char::from(vk as u8).to_string(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x6A => "Num*".into(),
        0x6B => "Num+".into(),
        0x6D => "Num-".into(),
        0x6E => "Num.".into(),
        0x6F => "Num/".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x90 => "NumLock".into(),
        0x91 => "ScrollLock".into(),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        0x5D => "Menu".into(),
        _ => format!("0x{vk:02X}"),
    }
}
