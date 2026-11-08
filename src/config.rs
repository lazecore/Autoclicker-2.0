//! Settings persistence — zero dependencies, human-readable INI.
//!
//! Location: `%APPDATA%\Autoclicker2\config.ini`. If the file is missing or
//! corrupt, defaults are used; the app can never fail to start because of it
//! (every read/write is guarded with `Option` / `let _ =`).

use std::path::PathBuf;

use crate::sys;

/// All user settings. `PartialEq` is needed for the "did it change?" check:
/// the UI pushes settings to the engine every frame, but saving only happens
/// on a real change (debounced).
#[derive(Clone, PartialEq)]
pub struct Settings {
    // ── Click interval (components) ───────────────────────────────────────
    pub interval_h: u32,
    pub interval_m: u32,
    pub interval_s: u32,
    pub interval_ms: u32,

    // ── Random offset ─────────────────────────────────────────────────────
    pub jitter_enabled: bool,
    /// ± random offset (ms).
    pub jitter_ms: f32,

    // ── Click ─────────────────────────────────────────────────────────────
    /// 0 = left, 1 = right, 2 = middle.
    pub button: u8,
    /// Send a double click (down/up ×2) on every interval.
    pub double_click: bool,

    // ── Repeat ────────────────────────────────────────────────────────────
    /// true → until stopped; false → `repeat_count` times.
    pub repeat_forever: bool,
    pub repeat_count: u32,

    // ── Cursor position ───────────────────────────────────────────────────
    pub fixed_pos: bool,
    pub pos_x: i32,
    pub pos_y: i32,

    // ── Advanced ──────────────────────────────────────────────────────────
    /// Wait before starting (ms) — time to focus the target window.
    pub start_delay_ms: u32,
    /// Maximum session time in seconds (0 = unlimited).
    pub limit_secs: u32,
    pub always_on_top: bool,

    // ── Hotkeys (Windows virtual-key codes) ───────────────────────────────
    pub hotkey_toggle: u32,
    pub hotkey_stop: u32,
    pub hotkey_pick: u32,
    pub hotkey_macro: u32,

    // ── Macro (a key-bound burst of clicks) ───────────────────────────────
    /// 0 = left, 1 = right.
    pub macro_button: u8,
    pub macro_clicks: u32,
    pub macro_interval_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_h: 0,
            interval_m: 0,
            interval_s: 0,
            // 100 ms ≈ 10 clicks/sec.
            interval_ms: 100,
            jitter_enabled: true,
            jitter_ms: 15.0,
            button: 0,
            double_click: false,
            repeat_forever: true,
            repeat_count: 100,
            fixed_pos: false,
            pos_x: 0,
            pos_y: 0,
            start_delay_ms: 0,
            limit_secs: 0,
            always_on_top: false,
            hotkey_toggle: sys::VK_F6 as u32,
            hotkey_stop: sys::VK_F7 as u32,
            hotkey_pick: sys::VK_F8 as u32,
            hotkey_macro: sys::VK_R,
            macro_button: 0,
            macro_clicks: 10,
            macro_interval_ms: 100,
        }
    }
}

/// `%APPDATA%\Autoclicker2` (falls back to the temp directory).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Autoclicker2")
}

fn config_path() -> PathBuf {
    data_dir().join("config.ini")
}

impl Settings {
    /// Total click interval (ms).
    pub fn total_interval_ms(&self) -> u32 {
        self.interval_h
            .saturating_mul(3_600_000)
            .saturating_add(self.interval_m.saturating_mul(60_000))
            .saturating_add(self.interval_s.saturating_mul(1_000))
            .saturating_add(self.interval_ms)
    }

    /// Interval sent to the engine (µs). At least 1 ms.
    pub fn interval_us(&self) -> u32 {
        self.total_interval_ms().max(1).saturating_mul(1_000)
    }

    /// Clicks per second (for the `≈ N clicks/sec` readout).
    pub fn cps(&self) -> f32 {
        1_000_000.0 / self.interval_us() as f32
    }

    /// Reads from disk; missing/corrupt fields keep their defaults.
    pub fn load() -> Self {
        let mut s = Self::default();
        let Ok(text) = std::fs::read_to_string(config_path()) else {
            return s;
        };

        let mut legacy_cps: Option<f32> = None;
        let mut saw_interval = false;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "interval_h" => {
                    s.interval_h = value.parse().unwrap_or(s.interval_h);
                    saw_interval = true;
                }
                "interval_m" => {
                    s.interval_m = value.parse().unwrap_or(s.interval_m);
                    saw_interval = true;
                }
                "interval_s" => {
                    s.interval_s = value.parse().unwrap_or(s.interval_s);
                    saw_interval = true;
                }
                "interval_ms" => {
                    s.interval_ms = value.parse().unwrap_or(s.interval_ms);
                    saw_interval = true;
                }
                "cps" => legacy_cps = value.parse().ok(),
                "jitter_enabled" => {
                    s.jitter_enabled = parse_bool(value).unwrap_or(s.jitter_enabled)
                }
                "jitter_ms" => s.jitter_ms = value.parse().unwrap_or(s.jitter_ms),
                "button" => s.button = value.parse().unwrap_or(s.button),
                "double_click" => s.double_click = parse_bool(value).unwrap_or(s.double_click),
                "repeat_forever" => {
                    s.repeat_forever = parse_bool(value).unwrap_or(s.repeat_forever)
                }
                "repeat_count" => s.repeat_count = value.parse().unwrap_or(s.repeat_count),
                "limit_clicks" => {
                    // 2.x compatibility: 0 = unlimited.
                    if let Ok(n) = value.parse::<u32>() {
                        if n == 0 {
                            s.repeat_forever = true;
                        } else {
                            s.repeat_forever = false;
                            s.repeat_count = n;
                        }
                    }
                }
                "fixed_pos" => s.fixed_pos = parse_bool(value).unwrap_or(s.fixed_pos),
                "pos_x" => s.pos_x = value.parse().unwrap_or(s.pos_x),
                "pos_y" => s.pos_y = value.parse().unwrap_or(s.pos_y),
                "start_delay_ms" => s.start_delay_ms = value.parse().unwrap_or(s.start_delay_ms),
                "limit_secs" => s.limit_secs = value.parse().unwrap_or(s.limit_secs),
                "always_on_top" => s.always_on_top = parse_bool(value).unwrap_or(s.always_on_top),
                "hotkey_toggle" => s.hotkey_toggle = value.parse().unwrap_or(s.hotkey_toggle),
                "hotkey_stop" => s.hotkey_stop = value.parse().unwrap_or(s.hotkey_stop),
                "hotkey_pick" => s.hotkey_pick = value.parse().unwrap_or(s.hotkey_pick),
                "hotkey_macro" => s.hotkey_macro = value.parse().unwrap_or(s.hotkey_macro),
                "macro_button" => s.macro_button = value.parse().unwrap_or(s.macro_button),
                "macro_clicks" => s.macro_clicks = value.parse().unwrap_or(s.macro_clicks),
                "macro_interval_ms" => {
                    s.macro_interval_ms = value.parse().unwrap_or(s.macro_interval_ms)
                }
                _ => {}
            }
        }

        // An old 2.x file only had `cps=`: convert it to an interval.
        if !saw_interval {
            if let Some(cps) = legacy_cps.filter(|c| c.is_finite() && *c > 0.0) {
                let total = (1_000.0 / cps).round().max(1.0) as u32;
                s.interval_s = total / 1_000;
                s.interval_ms = total % 1_000;
            }
        }

        s.sanitize()
    }

    /// Clamps values into safe ranges (protects against hand-edited files).
    pub fn sanitize(mut self) -> Self {
        self.interval_h = self.interval_h.min(23);
        self.interval_m = self.interval_m.min(59);
        self.interval_s = self.interval_s.min(59);
        self.interval_ms = self.interval_ms.min(999);
        if self.total_interval_ms() == 0 {
            self.interval_ms = 1;
        }

        self.jitter_ms = if self.jitter_ms.is_finite() {
            self.jitter_ms.clamp(0.0, 200.0)
        } else {
            15.0
        };
        if self.button > 2 {
            self.button = 0;
        }
        self.repeat_count = self.repeat_count.clamp(1, 1_000_000);
        self.start_delay_ms = self.start_delay_ms.min(10_000);
        self.limit_secs = self.limit_secs.min(3_600);

        let d = Self::default();
        if !valid_hotkey(self.hotkey_toggle) {
            self.hotkey_toggle = d.hotkey_toggle;
        }
        if !valid_hotkey(self.hotkey_stop) {
            self.hotkey_stop = d.hotkey_stop;
        }
        if !valid_hotkey(self.hotkey_pick) {
            self.hotkey_pick = d.hotkey_pick;
        }
        if !valid_hotkey(self.hotkey_macro) {
            self.hotkey_macro = d.hotkey_macro;
        }
        // Never let two actions share one key: revert the duplicate to default.
        for slot in 1..=4u8 {
            if slot_duplicated(&self, slot) {
                match slot {
                    1 => self.hotkey_toggle = d.hotkey_toggle,
                    2 => self.hotkey_stop = d.hotkey_stop,
                    3 => self.hotkey_pick = d.hotkey_pick,
                    _ => self.hotkey_macro = d.hotkey_macro,
                }
            }
        }

        if self.macro_button > 1 {
            self.macro_button = 0;
        }
        self.macro_clicks = self.macro_clicks.clamp(1, 100_000);
        self.macro_interval_ms = self.macro_interval_ms.clamp(1, 10_000);
        self
    }

    /// Writes to disk; the app keeps running even if writing fails.
    pub fn save(&self) {
        let text = format!(
            "# Autoclicker 2.0 — settings\n\
             interval_h={}\n\
             interval_m={}\n\
             interval_s={}\n\
             interval_ms={}\n\
             jitter_enabled={}\n\
             jitter_ms={}\n\
             button={}\n\
             double_click={}\n\
             repeat_forever={}\n\
             repeat_count={}\n\
             fixed_pos={}\n\
             pos_x={}\n\
             pos_y={}\n\
             start_delay_ms={}\n\
             limit_secs={}\n\
             always_on_top={}\n\
             hotkey_toggle={}\n\
             hotkey_stop={}\n\
             hotkey_pick={}\n\
             hotkey_macro={}\n\
             macro_button={}\n\
             macro_clicks={}\n\
             macro_interval_ms={}\n",
            self.interval_h,
            self.interval_m,
            self.interval_s,
            self.interval_ms,
            self.jitter_enabled,
            self.jitter_ms,
            self.button,
            self.double_click,
            self.repeat_forever,
            self.repeat_count,
            self.fixed_pos,
            self.pos_x,
            self.pos_y,
            self.start_delay_ms,
            self.limit_secs,
            self.always_on_top,
            self.hotkey_toggle,
            self.hotkey_stop,
            self.hotkey_pick,
            self.hotkey_macro,
            self.macro_button,
            self.macro_clicks,
            self.macro_interval_ms,
        );
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(config_path(), text);
    }
}

/// Hand-edited INI booleans accept `True`, `FALSE`, `1`, `on`, etc.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// A valid assignable key? Mouse buttons (0x01/0x02/0x04) and non-functional
/// codes are excluded.
pub fn valid_hotkey(vk: u32) -> bool {
    (0x08..=0xFE).contains(&vk) && !matches!(vk, 0x0A | 0x0B | 0x5E)
}

/// The key selected for slot (1..=4) is also selected in another slot.
fn slot_duplicated(s: &Settings, slot: u8) -> bool {
    let keys = [
        s.hotkey_toggle,
        s.hotkey_stop,
        s.hotkey_pick,
        s.hotkey_macro,
    ];
    let idx = slot as usize - 1;
    let mine = keys[idx];
    keys.iter().enumerate().any(|(i, k)| i != idx && *k == mine)
}
