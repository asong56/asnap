// recorder.rs — Alt + ] (toggle)
//
// Start: interactive region selection → launch platform recorder subprocess.
// Stop:  send SIGINT / terminate to the subprocess; wait for it to mux and exit.
//
// The Child handle + output path are kept in a lazy Mutex so start/stop can be
// called from different threads.

use std::path::PathBuf;
use std::process::Child;
use std::sync::{Mutex, OnceLock};

use crate::RECORDING;
use crate::desktop;
use crate::screenshot::Region;

use std::sync::atomic::Ordering;

fn state() -> &'static Mutex<Option<(Child, PathBuf)>> {
    static S: OnceLock<Mutex<Option<(Child, PathBuf)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub fn start() {
    let region = match select_region() {
        Some(r) if r.w > 4 && r.h > 4 => r,
        _ => {
            eprintln!("[asnap/recorder] region selection cancelled");
            return;
        }
    };

    let out = desktop::output_path("mp4");

    let child = match spawn_recorder(&region, &out) {
        Some(c) => c,
        None => {
            eprintln!("[asnap/recorder] failed to start recorder");
            return;
        }
    };

    *state().lock().unwrap() = Some((child, out.clone()));
    RECORDING.store(true, Ordering::SeqCst);
    eprintln!("[asnap/recorder] recording started → {}", out.display());
}

pub fn stop() {
    let mut guard = state().lock().unwrap();
    if let Some((mut child, path)) = guard.take() {
        terminate_recorder(&mut child);
        let _ = child.wait();
        RECORDING.store(false, Ordering::SeqCst);
        eprintln!("[asnap/recorder] recording saved → {}", path.display());
    } else {
        eprintln!("[asnap/recorder] no active recording to stop");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Region selection (reuse same helpers as screenshot.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn select_region() -> Option<Region> {
    crate::screenshot::select_region_pub()
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform: spawn recorder
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn spawn_recorder(r: &Region, out: &PathBuf) -> Option<Child> {
    // screencapture -v -R x,y,w,h output.mp4
    // Recording stops when we SIGINT the process.
    use std::process::{Command, Stdio};
    Command::new("screencapture")
        .args([
            "-v",
            "-R", &format!("{},{},{},{}", r.x, r.y, r.w, r.h),
            out.to_str()?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(target_os = "linux")]
fn spawn_recorder(r: &Region, out: &PathBuf) -> Option<Child> {
    use std::process::{Command, Stdio};
    // Try wl-screenrec first (GPU-accelerated), fall back to wf-recorder.
    let geom = format!("{},{} {}x{}", r.x, r.y, r.w, r.h);

    let child = Command::new("wl-screenrec")
        .args(["--geometry", &geom, "--file", out.to_str()?])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    if let Ok(c) = child { return Some(c); }

    // Fallback: wf-recorder
    Command::new("wf-recorder")
        .args(["-g", &geom, "-f", out.to_str()?])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(target_os = "windows")]
fn spawn_recorder(r: &Region, out: &PathBuf) -> Option<Child> {
    use std::process::{Command, Stdio};
    // ffmpeg gdigrab with crop filter
    Command::new("ffmpeg")
        .args([
            "-f",     "gdigrab",
            "-i",     "desktop",
            "-vf",    &format!("crop={}:{}:{}:{}", r.w, r.h, r.x, r.y),
            "-c:v",   "libx264",
            "-preset","ultrafast",
            "-pix_fmt","yuv420p",
            "-y",
            out.to_str()?,
        ])
        .stdin(Stdio::piped())   // we send 'q' to stop gracefully
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform: terminate recorder
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminate_recorder(child: &mut Child) {
    // Send SIGINT so the recorder can flush and mux cleanly.
    let pid = child.id();
    unsafe { libc::kill(pid as i32, libc::SIGINT); }
    // Give it up to 8 seconds to finalise.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        if child.try_wait().map(|s| s.is_some()).unwrap_or(true) { break; }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(target_os = "windows")]
fn terminate_recorder(child: &mut Child) {
    // Send 'q\n' to ffmpeg stdin for a clean stop.
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"q\n");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        if child.try_wait().map(|s| s.is_some()).unwrap_or(true) { break; }
        if std::time::Instant::now() > deadline { let _ = child.kill(); break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// libc for Unix signal
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod libc {
    extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
    pub const SIGINT: i32 = 2;
}
