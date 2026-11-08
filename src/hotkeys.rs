//! Global hotkeys — their own thread, fully independent of the UI.
//!
//! WHY A SEPARATE THREAD?
//! Polling keys from egui's `logic()` stops as soon as the window is minimized
//! or rendering pauses — exactly when a game is in the foreground. This thread
//! polls at a fixed 6 ms cadence, so hotkeys keep working no matter what the
//! window is doing.
//!
//! Hotkeys are assignable: the UI only writes "which slot is waiting for a
//! key" (`capture_slot`); this thread waits for the key, then reports the
//! virtual-key code back through `captured_vk`.
//!
//! WHY NOT A HOOK?
//! `SetWindowsHookEx(WH_KEYBOARD_LL)` is the signature of a keylogger and a
//! common antivirus false positive. `GetAsyncKeyState` works system-wide
//! (focus independent), installs no hook, and edge detection is done with our
//! own previous-state flags.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::{Duration, Instant};

use crate::diag;
use crate::engine::Shared;
use crate::sys;

/// Only trigger on the rising edge, never while the key is held.
const POLL_MS: u64 = 6;

/// Grace period after entering capture mode: a key still held from the click
/// that opened the dialog must not be assigned by accident.
const CAPTURE_SETTLE_MS: u64 = 180;

/// After a successful capture, ignore input briefly so the freshly assigned
/// key does not immediately fire its action.
const CAPTURE_IGNORE_MS: u64 = 350;

const VK_ESCAPE: u32 = 0x1B;

pub fn spawn(shared: Arc<Shared>) {
    let _ = thread::Builder::new()
        .name("hotkeys".into())
        .stack_size(32 * 1024)
        .spawn(move || run(shared));
}

fn load_keys(shared: &Shared) -> [u32; 4] {
    [
        shared.hotkey_toggle.load(Relaxed),
        shared.hotkey_stop.load(Relaxed),
        shared.hotkey_pick.load(Relaxed),
        shared.hotkey_macro.load(Relaxed),
    ]
}

/// Rising edge: the key is newly down (`down` + previous state up) OR the
/// press fell between two polls (`pressed` bit). The second case catches taps
/// that are shorter than the poll interval.
fn edge(now: sys::KeyState, prev_down: bool) -> bool {
    (now.down && !prev_down) || now.pressed
}

#[derive(Clone, Copy)]
enum Capture {
    Key(u32),
    Cancelled,
}

fn run(shared: Arc<Shared>) {
    let mut prev = [false; 4];
    let mut live = load_keys(&shared);
    let mut ignore_until = Instant::now();

    while !shared.shutdown.load(Relaxed) {
        // ── Capture mode ──────────────────────────────────────────────────
        if shared.capture_slot.load(Relaxed) != 0 {
            match capture_key(&shared) {
                Capture::Key(VK_ESCAPE) | Capture::Cancelled => {
                    shared.captured_vk.store(-1, Relaxed);
                }
                Capture::Key(vk) => {
                    diag::log(format!("hotkey capture -> vk={vk}"));
                    shared.captured_vk.store(vk as i32, Relaxed);
                }
            }
            shared.capture_slot.store(0, Relaxed);
            prev = [false; 4];
            ignore_until = Instant::now() + Duration::from_millis(CAPTURE_IGNORE_MS);
            continue;
        }

        // Keep the edge state in sync while ignoring input, so a still-held
        // key does not fire the moment the guard expires.
        if Instant::now() < ignore_until {
            let keys = load_keys(&shared);
            live = keys;
            for (index, vk) in keys.iter().enumerate() {
                prev[index] = sys::key_state(*vk as i32).down;
            }
            thread::sleep(Duration::from_millis(POLL_MS));
            continue;
        }

        let keys = load_keys(&shared);
        // The user changed a binding: reset edge states.
        if keys != live {
            live = keys;
            prev = [false; 4];
        }

        let states = [
            sys::key_state(keys[0] as i32),
            sys::key_state(keys[1] as i32),
            sys::key_state(keys[2] as i32),
            sys::key_state(keys[3] as i32),
        ];

        // 1) Start / stop.
        if edge(states[0], prev[0]) {
            shared.toggle();
            diag::log(format!(
                "hotkey toggle (vk={}) -> running={}",
                keys[0],
                shared.is_running()
            ));
        }

        // 2) Stop: stops clicking and cancels the macro. Does NOT quit the app.
        if edge(states[1], prev[1]) {
            shared.stop();
            shared.macro_cancel.store(true, Relaxed);
            diag::log("hotkey stop".to_string());
        }

        // 3) Use the cursor's current position as the fixed click point.
        if edge(states[2], prev[2]) {
            if let Some((x, y)) = sys::cursor_pos() {
                shared.pos_x.store(x, Relaxed);
                shared.pos_y.store(y, Relaxed);
                shared.fixed_pos.store(true, Relaxed);
                shared.capture_pulse.fetch_add(1, Relaxed);
                diag::log(format!("hotkey pick pos -> {x},{y}"));
            }
        }

        // 4) Run the macro.
        if edge(states[3], prev[3]) {
            shared.macro_request.store(true, Relaxed);
            diag::log(format!("hotkey macro (vk={})", keys[3]));
        }

        for index in 0..4 {
            prev[index] = states[index].down;
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// Waits (after the settle period) for the first key pressed, or for the slot
/// to be cancelled by the UI / shutdown.
fn capture_key(shared: &Shared) -> Capture {
    let settle_end = Instant::now() + Duration::from_millis(CAPTURE_SETTLE_MS);
    while Instant::now() < settle_end {
        if shared.shutdown.load(Relaxed) || shared.capture_slot.load(Relaxed) == 0 {
            return Capture::Cancelled;
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }

    loop {
        if shared.shutdown.load(Relaxed) || shared.capture_slot.load(Relaxed) == 0 {
            return Capture::Cancelled;
        }
        if let Some(vk) = scan_pressed_key() {
            return Capture::Key(vk);
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// Scans the virtual-key space for a press. Mouse buttons, modifiers and the
/// Windows keys are skipped so they cannot be assigned as hotkeys.
fn scan_pressed_key() -> Option<u32> {
    for vk in (0x08u32..=0xFEu32).rev() {
        if matches!(vk, 0x01 | 0x02 | 0x04 | 0x10 | 0x11 | 0x12 | 0x5B | 0x5C) {
            continue;
        }
        if sys::key_state(vk as i32).pressed {
            return Some(vk);
        }
    }
    None
}
