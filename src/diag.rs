//! Optional diagnostic log.
//!
//! Release builds have no console, so `eprintln!` would be invisible. Enable
//! `AUTOCLICKER_LOG=1` and messages are appended to
//! `%APPDATA%\Autoclicker2\debug.log`. In normal use no file is opened: the
//! cost is a single `OnceLock` check.

use std::io::Write;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("AUTOCLICKER_LOG").is_some())
}

pub fn log(msg: String) {
    if !enabled() {
        return;
    }
    let dir = crate::config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("debug.log"))
    {
        let _ = writeln!(f, "[pid {}] {msg}", std::process::id());
    }
}
