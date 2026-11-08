//! Macro: pressing the assigned key performs N clicks at a fixed interval.
//!
//! This is a second worker thread, independent of the main engine:
//! * It can run while the main autoclicker is clicking (if the user wants).
//! * The macro is finite: it clicks `macro_clicks` times and ends — unlike the
//!   main autoclicker's "until stopped" behaviour.
//! * Every click checks the cancel/shutdown flags; a button is never left
//!   held when the macro stops (`release_all`).

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::{Duration, Instant};

use crate::diag;
use crate::engine::Shared;
use crate::sys;

/// Idle poll interval for the macro worker.
const IDLE_POLL_MS: u64 = 6;

/// How long the button is held during a macro click (µs).
const PRESS_US: u64 = 35_000;

pub fn spawn(shared: Arc<Shared>) {
    let _ = thread::Builder::new()
        .name("macro".into())
        .stack_size(64 * 1024)
        .spawn(move || run(shared));
}

fn run(shared: Arc<Shared>) {
    while !shared.shutdown.load(Relaxed) {
        if shared.macro_request.swap(false, Relaxed) {
            execute(&shared);
        }
        thread::sleep(Duration::from_millis(IDLE_POLL_MS));
    }
    sys::release_all();
}

fn execute(shared: &Shared) {
    let count = shared.macro_clicks.load(Relaxed).max(1);
    let interval_us = shared.macro_interval_us.load(Relaxed).max(1_000) as u64;
    let button = shared.macro_button.load(Relaxed).min(1);

    // The press duration must not exceed the interval (very fast macros).
    let press_us = PRESS_US.min((interval_us as f64 * 0.6) as u64).max(1_000);

    shared.macro_cancel.store(false, Relaxed);
    shared.macro_progress.store(0, Relaxed);
    shared.macro_active.store(true, Relaxed);
    sys::timer_resolution_1ms(true);

    diag::log(format!(
        "macro: start count={} interval={}us button={}",
        count, interval_us, button
    ));

    // Absolute timeline: sleep overshoot does not accumulate.
    let mut next = Instant::now();
    let mut done = 0u32;

    for _ in 0..count {
        if shared.shutdown.load(Relaxed) || shared.macro_cancel.load(Relaxed) {
            diag::log("macro: cancelled".to_string());
            break;
        }

        sys::precise_sleep_at(next);

        sys::button_down(button);
        sys::precise_sleep(Duration::from_micros(press_us));
        sys::button_up(button);

        done += 1;
        shared.macro_progress.store(done, Relaxed);

        next += Duration::from_micros(interval_us);
        let floor = Instant::now() + Duration::from_micros(200);
        if next < floor {
            next = floor;
        }
    }

    // Safety: never leave a button held, even if cancelled.
    sys::release_all();
    sys::timer_resolution_1ms(false);
    shared.macro_active.store(false, Relaxed);
    shared.macro_cancel.store(false, Relaxed);
    diag::log(format!("macro: done {} clicks", done));
}
