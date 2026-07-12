// asnap — 幽灵快照
// Zero-UI, hotkey-driven screen-capture daemon.
//
// Hotkeys (right-hand symbol cluster):
//   Alt + [   → smart screenshot  (static or long/scrolling)
//   Alt + ]   → region recording  (toggle start / stop)
//   Alt + \   → native OCR        (text → clipboard + .txt on Desktop)

mod desktop;
mod ocr;
mod recorder;
mod screenshot;
mod stitch;

use rdev::{listen, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;

// ── Shared atomic state (written by rdev thread, read by worker threads) ──────

/// Set true while a screenshot region-selection is in progress so that
/// scroll events are accumulated rather than discarded.
pub static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Running sum of vertical scroll wheel deltas received while CAPTURE_ACTIVE.
/// A negative total means the user scrolled *down* (content moved up to reveal
/// more below); a positive total means scrolled *up*.
pub static SCROLL_DELTA: AtomicI64 = AtomicI64::new(0);

/// True while a screen-recording subprocess is running.
pub static RECORDING: AtomicBool = AtomicBool::new(false);

/// True while the Alt / Option key is held.
pub static ALT_HELD: AtomicBool = AtomicBool::new(false);

fn main() {
    eprintln!("╔══════════════════════════════════════════╗");
    eprintln!("║  asnap  幽灵快照  v{}              ║", env!("CARGO_PKG_VERSION"));
    eprintln!("╠══════════════════════════════════════════╣");
    eprintln!("║  Alt + [   screenshot / long-screenshot  ║");
    eprintln!("║  Alt + ]   start / stop recording        ║");
    eprintln!("║  Alt + \\   OCR → clipboard + .txt        ║");
    eprintln!("║  Ctrl-C    quit                          ║");
    eprintln!("╚══════════════════════════════════════════╝");

    #[cfg(target_os = "macos")]
    eprintln!("[asnap] macOS: grant Accessibility access when prompted");

    #[cfg(target_os = "linux")]
    eprintln!("[asnap] Linux: ensure you are in the 'input' group for hotkeys");

    if let Err(e) = listen(handle_event) {
        eprintln!("[asnap] Failed to register global listener: {:?}", e);
        eprintln!("[asnap] Check Accessibility / input-group permissions and retry.");
        std::process::exit(1);
    }
}

fn handle_event(event: Event) {
    match event.event_type {
        // ── Modifier tracking ──────────────────────────────────────────────
        EventType::KeyPress(Key::Alt) => {
            ALT_HELD.store(true, Ordering::SeqCst);
        }
        EventType::KeyRelease(Key::Alt) => {
            ALT_HELD.store(false, Ordering::SeqCst);
        }

        // ── Scroll accumulation (for long-screenshot) ──────────────────────
        EventType::Wheel { delta_y, .. } => {
            if CAPTURE_ACTIVE.load(Ordering::SeqCst) {
                SCROLL_DELTA.fetch_add(delta_y, Ordering::SeqCst);
            }
        }

        // ── Alt + [  →  screenshot ─────────────────────────────────────────
        EventType::KeyPress(Key::LeftBracket) => {
            if ALT_HELD.load(Ordering::SeqCst) && !CAPTURE_ACTIVE.load(Ordering::SeqCst) {
                thread::spawn(screenshot::run);
            }
        }

        // ── Alt + ]  →  toggle recording ──────────────────────────────────
        EventType::KeyPress(Key::RightBracket) => {
            if ALT_HELD.load(Ordering::SeqCst) {
                if RECORDING.load(Ordering::SeqCst) {
                    thread::spawn(recorder::stop);
                } else {
                    thread::spawn(recorder::start);
                }
            }
        }

        // ── Alt + \  →  OCR ───────────────────────────────────────────────
        EventType::KeyPress(Key::BackSlash) => {
            if ALT_HELD.load(Ordering::SeqCst) {
                thread::spawn(ocr::run);
            }
        }

        _ => {}
    }
}
