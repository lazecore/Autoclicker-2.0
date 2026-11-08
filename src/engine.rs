//! Click engine: shared atomic state + a single-task worker thread.
//!
//! Design decisions:
//! * Sharing is atomic, not `Mutex`: no locking or contention in the hot loop.
//! * `Relaxed` is deliberate: no heap data is "published" here, only
//!   independent flags and counters need atomicity.
//! * The worker never waits for the UI: hotkeys (`hotkeys.rs`) and the engine
//!   run on their own threads, so clicking and hotkeys keep working even when
//!   the window is minimized or unfocused.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, RngExt};

use crate::config::Settings;
use crate::diag;
use crate::sys;

// ── Stop reasons (shown to the user in the UI) ────────────────────────────
pub const STOP_NONE: u8 = 0;
pub const STOP_CLICK_LIMIT: u8 = 1;
pub const STOP_TIME_LIMIT: u8 = 2;

// ── Humanization parameters ───────────────────────────────────────────────
/// ~1.5% of clicks are stretched 1.8x–4.0x. A perfectly steady rhythm — even
/// with jitter — is the strongest "bot" signal in the frequency spectrum.
const HESITATION_PROB: f64 = 0.015;
const HESITATION_MIN: f64 = 1.8;
const HESITATION_MAX: f64 = 4.0;

/// A real finger holds the button for ~30–90 ms. A DOWN+UP pair in the same
/// millisecond is a bot signature in input-timing logs.
const PRESS_MIN_US: f64 = 30_000.0;
const PRESS_MAX_US: f64 = 90_000.0;

/// Lower bound for the interval (µs): protects against zero/negative delays.
const MIN_INTERVAL_US: f64 = 800.0;

/// Idle poll interval. Hotkeys run on their own thread, so this can be
/// generous; CPU usage stays negligible.
const IDLE_POLL_MS: u64 = 20;

/// Countdown update step.
const COUNTDOWN_STEP_MS: u64 = 15;

/// All state shared between the UI thread and the engine.
pub struct Shared {
    /// Is the click loop active?
    pub running: AtomicBool,
    /// Is the application shutting down?
    pub shutdown: AtomicBool,
    /// Average click interval (µs) = 1_000_000 / CPS.
    pub interval_us: AtomicU32,
    /// ± random offset (µs).
    pub jitter_us: AtomicU32,
    /// 0 = left, 1 = right, 2 = middle.
    pub button: AtomicU8,
    /// Send a double click on every interval.
    pub double_click: AtomicBool,
    /// Wait before starting (ms).
    pub start_delay_ms: AtomicU32,
    /// Click limit per session (0 = unlimited).
    pub limit_clicks: AtomicU32,
    /// Time limit per session in seconds (0 = unlimited).
    pub limit_secs: AtomicU32,
    /// Fixed-position mode.
    pub fixed_pos: AtomicBool,
    pub pos_x: AtomicI32,
    pub pos_y: AtomicI32,
    /// Total clicks over the app's lifetime.
    pub total_clicks: AtomicU64,
    /// Clicks in this session.
    pub session_clicks: AtomicU64,
    /// Remaining countdown time (ms); 0 → clicking / not waiting.
    pub countdown_ms: AtomicU32,
    /// Last stop reason (STOP_*).
    pub stop_reason: AtomicU8,
    /// Incremented when a position is picked — the UI shows brief feedback.
    pub capture_pulse: AtomicU64,

    // ── User-assignable hotkeys (Windows VK codes) ───────────────────────
    pub hotkey_toggle: AtomicU32,
    pub hotkey_stop: AtomicU32,
    pub hotkey_pick: AtomicU32,
    pub hotkey_macro: AtomicU32,
    /// Capture mode: 0 = off, 1..=4 = waiting for a key for that action.
    pub capture_slot: AtomicU8,
    /// Captured key: -1 = cancelled, 0 = no result yet, otherwise a VK code.
    pub captured_vk: AtomicI32,

    // ── Macro (a key-bound, finite click burst) ──────────────────────────
    /// The UI or a hotkey asked the engine to run the macro; consumed here.
    pub macro_request: AtomicBool,
    /// Cancel the macro (stop key or UI button).
    pub macro_cancel: AtomicBool,
    /// Is the macro running right now?
    pub macro_active: AtomicBool,
    /// 0 = left, 1 = right.
    pub macro_button: AtomicU8,
    pub macro_clicks: AtomicU32,
    pub macro_interval_us: AtomicU32,
    /// Completed macro clicks (for the progress bar).
    pub macro_progress: AtomicU32,
}

impl Shared {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            running: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            interval_us: AtomicU32::new(s.interval_us()),
            jitter_us: AtomicU32::new((s.jitter_ms * 1_000.0) as u32),
            button: AtomicU8::new(s.button),
            double_click: AtomicBool::new(s.double_click),
            start_delay_ms: AtomicU32::new(s.start_delay_ms),
            limit_clicks: AtomicU32::new(if s.repeat_forever { 0 } else { s.repeat_count }),
            limit_secs: AtomicU32::new(s.limit_secs),
            fixed_pos: AtomicBool::new(s.fixed_pos),
            pos_x: AtomicI32::new(s.pos_x),
            pos_y: AtomicI32::new(s.pos_y),
            total_clicks: AtomicU64::new(0),
            session_clicks: AtomicU64::new(0),
            countdown_ms: AtomicU32::new(0),
            stop_reason: AtomicU8::new(STOP_NONE),
            capture_pulse: AtomicU64::new(0),
            hotkey_toggle: AtomicU32::new(s.hotkey_toggle),
            hotkey_stop: AtomicU32::new(s.hotkey_stop),
            hotkey_pick: AtomicU32::new(s.hotkey_pick),
            hotkey_macro: AtomicU32::new(s.hotkey_macro),
            capture_slot: AtomicU8::new(0),
            captured_vk: AtomicI32::new(0),
            macro_request: AtomicBool::new(false),
            macro_cancel: AtomicBool::new(false),
            macro_active: AtomicBool::new(false),
            macro_button: AtomicU8::new(s.macro_button),
            macro_clicks: AtomicU32::new(s.macro_clicks.max(1)),
            macro_interval_us: AtomicU32::new(s.macro_interval_ms.max(1) * 1_000),
            macro_progress: AtomicU32::new(0),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Relaxed)
    }

    /// Start/stop. A new session clears the previous stop reason so the UI
    /// does not keep showing a stale "limit reached" notice.
    pub fn toggle(&self) {
        if self.is_running() {
            self.running.store(false, Relaxed);
        } else {
            self.stop_reason.store(STOP_NONE, Relaxed);
            self.running.store(true, Relaxed);
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Relaxed);
    }
}

/// Starts the worker thread.
pub fn spawn(shared: Arc<Shared>) {
    // 64 KB stack is enough: the worker has no recursion or large heap work.
    let _ = thread::Builder::new()
        .name("click-engine".into())
        .stack_size(64 * 1024)
        .spawn(move || run(shared));
}

fn run(shared: Arc<Shared>) {
    // Thread-local ChaCha12; a few draws per click, zero allocation.
    let mut rng = rand::rng();
    // Timer resolution is raised ONLY while clicking, so idling does not
    // waste power system-wide.
    let mut timer_raised = false;
    let mut active = false;
    let mut session_start = Instant::now();
    let mut session_clicks: u64 = 0;

    // Diagnostics: actual click rate per second (only with AUTOCLICKER_LOG).
    let mut rate_window = Instant::now();
    let mut rate_clicks: u32 = 0;

    // ABSOLUTE time of the next click. Keeping a target instant instead of
    // sleeping relatively prevents Sleep overshoot from accumulating and
    // dragging the target rate down.
    let mut next_click = Instant::now();

    loop {
        if shared.shutdown.load(Relaxed) {
            break;
        }

        if !shared.is_running() {
            if timer_raised {
                sys::timer_resolution_1ms(false);
                timer_raised = false;
            }
            if active {
                active = false;
                session_clicks = 0;
                shared.session_clicks.store(0, Relaxed);
                shared.countdown_ms.store(0, Relaxed);
                // Stop safety: never leave a button held.
                sys::release_all();
            }
            thread::sleep(Duration::from_millis(IDLE_POLL_MS));
            continue;
        }

        if !timer_raised {
            sys::timer_resolution_1ms(true);
            timer_raised = true;
        }

        // ── Session start: countdown ──────────────────────────────────────
        if !active {
            active = true;
            session_clicks = 0;
            shared.session_clicks.store(0, Relaxed);

            let delay = shared.start_delay_ms.load(Relaxed) as u64;
            diag::log(format!(
                "engine: session start (delay={}ms, interval={}us, jitter={}us, button={}, fixed={})",
                delay,
                shared.interval_us.load(Relaxed),
                shared.jitter_us.load(Relaxed),
                shared.button.load(Relaxed),
                shared.fixed_pos.load(Relaxed)
            ));
            if delay > 0 {
                let until = Instant::now() + Duration::from_millis(delay);
                while Instant::now() < until {
                    if shared.shutdown.load(Relaxed) || !shared.is_running() {
                        break;
                    }
                    let left = until.saturating_duration_since(Instant::now());
                    shared
                        .countdown_ms
                        .store(left.as_millis() as u32 + 1, Relaxed);
                    thread::sleep(Duration::from_millis(COUNTDOWN_STEP_MS));
                }
                shared.countdown_ms.store(0, Relaxed);
            }
            session_start = Instant::now();
            next_click = Instant::now();
            if shared.shutdown.load(Relaxed) || !shared.is_running() {
                continue;
            }
        }

        // ── Limits ────────────────────────────────────────────────────────
        let click_limit = shared.limit_clicks.load(Relaxed);
        if click_limit > 0 && session_clicks >= click_limit as u64 {
            diag::log(format!("engine: click limit reached ({})", session_clicks));
            shared.stop_reason.store(STOP_CLICK_LIMIT, Relaxed);
            shared.running.store(false, Relaxed);
            continue;
        }
        let time_limit = shared.limit_secs.load(Relaxed);
        if time_limit > 0 && session_start.elapsed() >= Duration::from_secs(time_limit as u64) {
            shared.stop_reason.store(STOP_TIME_LIMIT, Relaxed);
            shared.running.store(false, Relaxed);
            continue;
        }

        // ── One interval: one or two clicks ───────────────────────────────
        // Settings are read live on every interval: slider changes take effect
        // immediately, no restart required.
        let base_us = shared.interval_us.load(Relaxed) as f64;
        let jitter_us = shared.jitter_us.load(Relaxed) as f64;
        let button = shared.button.load(Relaxed);
        let burst = if shared.double_click.load(Relaxed) {
            2u32
        } else {
            1
        };

        let interval = humanized_interval(&mut rng, base_us, jitter_us);
        let per_click = interval / burst as f64;
        let press = press_duration(&mut rng, per_click);

        let gap = (per_click - press).max(MIN_INTERVAL_US);

        // Fixed-position mode: move the cursor to the target before each
        // burst. Clicks go wherever the cursor is, so this pins the location.
        if shared.fixed_pos.load(Relaxed) {
            sys::set_cursor(shared.pos_x.load(Relaxed), shared.pos_y.load(Relaxed));
        }

        // Wait for the next click (absolute target).
        sys::precise_sleep_at(next_click);
        let burst_start = Instant::now();

        for index in 0..burst {
            rate_clicks += 1;
            let ok_down = sys::button_down(button);
            sys::precise_sleep(Duration::from_micros(press as u64));
            let ok_up = sys::button_up(button);
            if session_clicks == 0 {
                // Only on the first click: did SendInput actually inject?
                diag::log(format!(
                    "engine: first click down_ok={} up_ok={} press={}us gap={}us",
                    ok_down, ok_up, press as u64, gap as u64
                ));
            }

            session_clicks += 1;
            shared.total_clicks.fetch_add(1, Relaxed);
            shared.session_clicks.store(session_clicks, Relaxed);

            // Gap between the two presses of a double-click.
            if index + 1 < burst {
                sys::precise_sleep(Duration::from_micros(gap as u64));
            }
        }

        // Advance the timeline: target = start of this burst + interval.
        // If we fell behind (system latency), resynchronize.
        next_click = burst_start + Duration::from_micros(interval as u64);
        let floor = Instant::now() + Duration::from_micros(MIN_INTERVAL_US as u64);
        if next_click < floor {
            next_click = floor;
        }

        let elapsed = rate_window.elapsed();
        if elapsed >= Duration::from_secs(1) {
            diag::log(format!(
                "engine: rate {} clicks in {}ms (interval={}us)",
                rate_clicks,
                elapsed.as_millis(),
                base_us as u32
            ));
            rate_window = Instant::now();
            rate_clicks = 0;
        }
    }

    // Shutdown safety: never leave a button held.
    sys::release_all();
    if timer_raised {
        sys::timer_resolution_1ms(false);
    }
}

/// Triangular jitter + random hesitation.
///
/// Averaging two uniform samples yields a triangular distribution: flat
/// (uniform) jitter produces a "square" histogram, while the triangular shape
/// clusters around the center like human reaction times.
fn humanized_interval<R: Rng + ?Sized>(rng: &mut R, base_us: f64, jitter_us: f64) -> f64 {
    let u1: f64 = rng.random();
    let u2: f64 = rng.random();
    let triangular = (u1 + u2) * 0.5;
    let offset = (triangular - 0.5) * 2.0 * jitter_us;
    let mut interval = (base_us + offset).max(MIN_INTERVAL_US);

    if jitter_us > 0.0 && rng.random::<f64>() < HESITATION_PROB {
        interval *= HESITATION_MIN + rng.random::<f64>() * (HESITATION_MAX - HESITATION_MIN);
    }
    interval
}

/// Duration between DOWN and UP; never more than 55% of the click interval,
/// so there is always a positive "release" gap between two clicks.
fn press_duration<R: Rng + ?Sized>(rng: &mut R, per_click_us: f64) -> f64 {
    let p: f64 = rng.random_range(PRESS_MIN_US..=PRESS_MAX_US);
    p.min(per_click_us * 0.55).max(1_000.0)
}
