# ⚡ Autoclicker 2.0

Ultra-light, humanized autoclicker for Windows — built with Rust, `SendInput`, and zero runtime dependencies.

> **One native `.exe`. No Python. No runtime. No hooks. No process injection.**

![platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue)
![lang](https://img.shields.io/badge/language-Rust-orange)
![license](https://img.shields.io/badge/license-MIT-green)

---

## Download

The prebuilt, ready-to-run executable is included in this repository:

**`Autoclicker-2.0.exe`** — portable single file, no installer, no runtime.

Windows SmartScreen may warn because the binary is not code-signed: choose **More info → Run anyway**.
(It is also attached to the GitHub *Releases* page.)

---

## Highlights

| | Typical Python autoclicker | **Autoclicker 2.0** |
|---|---|---|
| Input method | `pyautogui` / `mouse_event` (deprecated) | **`SendInput`** — hardware input stream |
| Timing | fixed interval (trivially detected) | **triangular jitter + hesitation + press duration** |
| Keyboard listening | low-level hook (`WH_KEYBOARD_LL`) | **`GetAsyncKeyState` polling** (no keylogger signature) |
| Antivirus false positives | common | minimized — only documented user32/winmm calls |
| Memory | 80–200 MB (interpreter) | single-digit-to-low-MB native binary |
| Garbage collection | interpreter GC pauses | none — zero-allocation hot loop |

- **Click interval** in hours / minutes / seconds / milliseconds, live `≈ N clicks/sec` readout
- **Random offset** — ±0–200 ms triangular jitter with occasional human hesitations
- **Buttons** left / right / middle, optional double-click bursts
- **Repeat** until stopped or a fixed click count, with progress
- **Cursor position** — follow the cursor or click a fixed, picked point
- **Macro** — assignable key, button, click count and interval
- **Assignable hotkeys** — 4 slots, captured from the keyboard, persisted
- **Live stats** — measured CPS, session clicks and lifetime total in the header
- **Fixed 520×730 window** with a smooth vertical scroll area and animated transitions
- Settings persisted automatically; dark amber theme
- **Advanced** (collapsed): start delay, session time limit, always on top, counter reset

Made by [lazecore](https://github.com/lazecore).

## The humanization engine (3 layers)

1. **Triangular jitter** — two uniform samples averaged. Unlike flat uniform noise, the histogram peaks in the center, like human reaction times.
2. **Hesitation** — ~1.5% of clicks are randomly stretched 1.8x–4.0x. A perfectly steady rhythm (even with jitter) is the strongest statistical "bot" signal.
3. **Press duration** — `DOWN` and `UP` are separated by 30–90 ms of randomness. Same-millisecond DOWN+UP pairs are a classic bot fingerprint in input-timing logs.

### Precision timing

Windows' default timer resolution (~15.6 ms) would wreck anything above ~60 CPS. The click engine raises it to 1 ms — **only while clicking** — and finishes every wait with a QPC-based spin window for sub-millisecond accuracy. Idle CPU cost: ~0%.

## Using it

1. Set **Click interval** (hours / minutes / seconds / ms). `0 s + 100 ms` = 10 clicks per second.
2. Pick the **mouse button** (left / right / middle) and **click type** (single / double).
3. **Click repeat** — *until stopped*, or exactly *N* clicks (a progress bar appears under Start).
4. Optionally switch **Cursor position** to a fixed point and press *Pick* (default **F8**), or type X/Y.
5. Press **Start** (default **F6**). The same key stops it; **F7** always stops.

### Macro (one key → a burst of clicks)

The macro is a one-shot script: it clicks a chosen button, a chosen number of times, at a chosen interval.

| Field | Meaning |
|---|---|
| Button | left or right |
| Clicks | how many clicks per run (e.g. 10) |
| Interval | ms between clicks (e.g. 100) |

Assign it to any key (default **R**) and press *Run macro*. It runs **once per keypress**, can be cancelled with the stop key, and is independent of the main clicker: the main engine can keep clicking while the macro fires, or either can be used alone.

## Hotkeys (global — work even when minimized)

Open **Hotkeys**, press **Change** next to an action, then press the key you want. **Esc** cancels. A key can only be assigned to one action; a duplicate is rejected.

| Slot | Default | Action |
|---|---|---|
| Start / Stop | **F6** | starts or stops clicking |
| Stop | **F7** | stops clicking **and** cancels a running macro |
| Pick position | **F8** | uses the current cursor position as the fixed click point |
| Macro | **R** | runs the macro once (button / count / interval from the Macro card) |

## Settings

File: `%APPDATA%\Autoclicker2\config.ini` — plain `key=value`, editable by hand.
Older 2.x files load fine: `cps=` is converted to an interval and `limit_clicks` to the repeat count.
Crash log (if a panic ever happens): `%APPDATA%\Autoclicker2\crash.log`.

## Build

```bash
cargo build --release
# → target/release/autoclicker.exe
```

Requirements: Rust 1.85+ (edition 2024). No other toolchain, SDK, or runtime.

The renderer is pinned to **glow (OpenGL)** and the `wgpu` backend is not used: on machines with certain Intel Vulkan drivers (`igvk64.dll`), eframe's default wgpu/Vulkan path crashes with an access violation. OpenGL is more stable there and yields a much smaller executable.

## Project layout

```
src/main.rs          entry point, fixed window, renderer selection, panic log
src/sys.rs           all Windows FFI (SendInput, cursor, keys, precise sleep)
src/engine.rs        atomic shared state + click worker thread
src/hotkeys.rs       frame-independent hotkey thread (assignable slots + capture)
src/macro_engine.rs  independent one-shot macro worker thread
src/config.rs        settings persistence (no external deps)
src/app.rs           egui/eframe UI
```

Threads: one click worker, one macro worker and one hotkey poller. None of them depends on the egui frame loop, so hotkeys and macros keep working while the window is minimized or unfocused.

## Ethics & disclaimer

This project is published for **educational purposes**: it demonstrates low-level Windows input simulation, precise timing, and statistical humanization in safe, idiomatic Rust. Using an autoclicker may violate the terms of service of games and other software — use it responsibly and at your own risk. The author is not responsible for any account actions or bans.

## Honest security notes

- `SendInput`-injected events carry the `LLMHF_INJECTED` flag, visible to kernel-level anti-cheats. No user-mode tool can avoid this; drivers require signing and are a far bigger AV red flag. This project does the legitimate best: proper API + statistically human timing.
- The app installs no hooks, reads no process memory, and writes only its own config file to `%APPDATA%` — there is simply nothing for antivirus engines to (falsely) flag.

## License

MIT — see [LICENSE](LICENSE).
