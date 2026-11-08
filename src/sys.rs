//! Windows API layer — all FFI in one place.
//!
//! Only documented, legitimate user32/winmm calls are used:
//! `SendInput` (input injection), `GetAsyncKeyState` (global key state),
//! `GetCursorPos`/`SetCursorPos` (cursor) and `timeBeginPeriod` (timer
//! resolution). There are no hooks, process injection or memory writes.
//!
//! A few functions are declared directly instead of through `windows-sys`:
//! pulling in extra feature flags for one-line FFI would be wasteful.

use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};

// ── Mouse button identifiers ───────────────────────────────────────────────
pub const BTN_LEFT: u8 = 0;
pub const BTN_RIGHT: u8 = 1;
pub const BTN_MIDDLE: u8 = 2;

// ── Virtual-key codes (Windows VK_*) ──────────────────────────────────────
/// Start / stop.
pub const VK_F6: i32 = 0x75;
/// Stop clicking and cancel the macro.
pub const VK_F7: i32 = 0x76;
/// Pick the fixed click position.
pub const VK_F8: i32 = 0x77;
/// Default macro key: R.
pub const VK_R: u32 = 0x52;

#[repr(C)]
struct RawPoint {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetCursorPos(point: *mut RawPoint) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn GetAsyncKeyState(vk: i32) -> i16;
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(ms: u32) -> u32;
    fn timeEndPeriod(ms: u32) -> u32;
}

/// Result of a single `GetAsyncKeyState` query.
#[derive(Clone, Copy, Default)]
pub struct KeyState {
    /// High bit (0x8000): the key is currently held.
    pub down: bool,
    /// Low bit (0x0001): the key was pressed since the previous query (even if
    /// it has already been released). This catches taps that fall between two
    /// polls.
    pub pressed: bool,
}

/// Current key state (system-wide — focus independent).
///
/// The `pressed` bit is valid until the next query for that key, so it must be
/// polled at a regular cadence — `hotkeys.rs` does this every 6 ms. The edge
/// decision is made by the caller together with its previous-state flag.
pub fn key_state(vk: i32) -> KeyState {
    // SAFETY: GetAsyncKeyState takes an integer and has no side effects.
    let bits = unsafe { GetAsyncKeyState(vk) as u16 };
    KeyState {
        down: bits & 0x8000 != 0,
        pressed: bits & 0x0001 != 0,
    }
}

/// Cursor position in screen coordinates.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = RawPoint { x: 0, y: 0 };
    // SAFETY: we pass a valid pointer to a stack-allocated RawPoint.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// Moves the cursor to the given screen coordinate (fixed-position mode).
pub fn set_cursor(x: i32, y: i32) {
    // SAFETY: only two integers; the return value reports failure, which is
    // safe to ignore here (a click still lands wherever the cursor is).
    unsafe {
        SetCursorPos(x, y);
    }
}

/// System timer granularity: `true` → 1 ms, `false` → system default.
pub fn timer_resolution_1ms(enable: bool) {
    // SAFETY: winmm is reference counted; every begin has a matching end
    // (the worker thread guarantees this with its `timer_raised` flag).
    unsafe {
        if enable {
            timeBeginPeriod(1);
        } else {
            timeEndPeriod(1);
        }
    }
}

/// Injects a single mouse event. `false` → the event was blocked (UIPI, e.g.
/// clicking into an elevated window).
fn send_mouse(flags: u32) -> bool {
    // INPUT contains a union; Windows expects unused fields to be zero.
    // dx=dy=0 → the cursor does not move, the click goes to its current spot.
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: 0,
        dwFlags: flags,
        time: 0, // 0 → the system timestamps it itself (most natural)
        dwExtraInfo: 0,
    };
    // SAFETY: `input` is fully initialized; the length is INPUT's real size.
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) == 1 }
}

/// Releases all three mouse buttons.
///
/// If a button stays "down" when a session ends, Windows ignores the next DOWN
/// events: the system then refuses clicks until it sees an UP (users read this
/// as "the autoclicker stopped working"). Every stop and every exit therefore
/// sends an explicit release; it is a no-op if the button is already up.
pub fn release_all() {
    for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
        button_up(button);
    }
}

pub fn button_down(button: u8) -> bool {
    send_mouse(match button {
        BTN_RIGHT => MOUSEEVENTF_RIGHTDOWN,
        BTN_MIDDLE => MOUSEEVENTF_MIDDLEDOWN,
        _ => MOUSEEVENTF_LEFTDOWN,
    })
}

pub fn button_up(button: u8) -> bool {
    send_mouse(match button {
        BTN_RIGHT => MOUSEEVENTF_RIGHTUP,
        BTN_MIDDLE => MOUSEEVENTF_MIDDLEUP,
        _ => MOUSEEVENTF_LEFTUP,
    })
}

/// Waits until an absolute deadline.
///
/// Keeping an absolute target instead of using relative sleeps prevents the
/// 1–2 ms overshoot of each `Sleep` call from accumulating; otherwise a 20 CPS
/// target would drift down to ~15 CPS.
pub fn precise_sleep_at(deadline: Instant) {
    const SPIN_WINDOW: Duration = Duration::from_micros(2_000);
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining > SPIN_WINDOW {
        std::thread::sleep(remaining - SPIN_WINDOW);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

/// Precise sleep that finishes the last ~2 ms with a QPC-based spin.
///
/// Windows' default `Sleep()` granularity is ~15.6 ms — fatal for anything
/// above ~60 CPS. `timeBeginPeriod(1)` brings it to 1 ms, and the remaining
/// tail is covered by spinning on `Instant` (QPC). Most of the duration is
/// still spent sleeping, so it stays CPU friendly.
pub fn precise_sleep(target: Duration) {
    precise_sleep_at(Instant::now() + target);
}
