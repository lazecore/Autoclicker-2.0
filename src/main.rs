// ═══════════════════════════════════════════════════════════════════════════
//!  AUTOCLICKER 2.0 — Rust + SendInput
//! ═══════════════════════════════════════════════════════════════════════════
//!
//!  Ultra-light, humanized Windows autoclicker.
//!
//!  MODULES
//!  ───────
//!   sys       → all Windows FFI (SendInput, keys/cursor, precise sleep)
//!   engine    → atomic shared state + click worker thread
//!   hotkeys   → UI-independent global hotkey thread
//!   config    → settings persistence under %APPDATA%
//!   app       → egui/eframe interface
//!
//!  Build    : `cargo build --release`  →  target/release/autoclicker.exe
//!  Defaults : F6 = start/stop, F7 = stop, F8 = pick position, R = macro
// ═══════════════════════════════════════════════════════════════════════════

// No console window in release builds; `eprintln!` stays visible in debug.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(windows))]
compile_error!("Autoclicker 2.0 targets Windows only (it requires the SendInput API).");

mod app;
mod config;
mod diag;
mod engine;
mod hotkeys;
mod macro_engine;
mod sys;

use eframe::egui;

fn main() -> eframe::Result<()> {
    install_panic_log();
    diag::log(format!("start v{}", env!("CARGO_PKG_VERSION")));

    let options = eframe::NativeOptions {
        // Explicit renderer choice — this is a fix:
        // the previous version used `Renderer::default()`, which in eframe
        // 0.37 is wgpu/Vulkan and crashed with 0xC0000005 on Intel's Vulkan
        // driver (igvk64.dll). glow/OpenGL is stable on this machine and
        // produces a much smaller binary.
        renderer: eframe::Renderer::Glow,
        // Fixed window: no full screen, no resizing.
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_TITLE)
            .with_inner_size([app::WINDOW_W, app::WINDOW_H])
            .with_min_inner_size([app::WINDOW_W, app::WINDOW_H])
            .with_max_inner_size([app::WINDOW_W, app::WINDOW_H])
            .with_resizable(false)
            .with_maximize_button(false),
        ..Default::default()
    };

    eframe::run_native(
        app::APP_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(app::AutoclickerApp::new(cc)))),
    )
}

/// Release builds have no console, so panic messages would disappear. Writing
/// them to `%APPDATA%\Autoclicker2\crash.log` makes "I clicked and nothing
/// happened" diagnosable in a GUI app.
fn install_panic_log() {
    std::panic::set_hook(Box::new(|info| {
        let dir = config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(
            dir.join("crash.log"),
            format!("unix_time={stamp}\n{info}\n\n{backtrace}\n"),
        );
    }));
}
