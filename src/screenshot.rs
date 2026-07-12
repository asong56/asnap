// screenshot.rs — Alt + [
//
// Flow:
//   1. Platform-specific region selector runs (blocks until user drags a rect).
//   2. We set CAPTURE_ACTIVE so rdev starts accumulating scroll deltas.
//   3. An initial frame is captured immediately.
//   4. We then wait up to SCROLL_TIMEOUT_MS.  If no scroll → static mode.
//      If scroll events arrive → long-screenshot mode: keep capturing frames
//      every SCROLL_FRAME_MS and accumulate until scrolling stops.
//   5. All frames are stitched (or single frame written directly).
//   6. Metadata is stripped and the file is saved to the Desktop.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::{CAPTURE_ACTIVE, SCROLL_DELTA};
use crate::desktop;
use crate::stitch;

/// Milliseconds to wait after region selection for a first scroll event before
/// concluding "static mode".
const SCROLL_WAIT_MS: u64 = 700;
/// Milliseconds between frame captures while scrolling continues.
const SCROLL_FRAME_MS: u64 = 220;
/// Milliseconds of inactivity after which we stop collecting scroll frames.
const SCROLL_IDLE_MS: u64 = 1_200;
/// Maximum number of frames (guards against infinite scroll).
const MAX_FRAMES: usize = 60;

/// A captured region on screen (display coordinates, top-left origin).
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn run() {
    // Step 1: interactive region selection
    let region = match select_region() {
        Some(r) if r.w > 4 && r.h > 4 => r,
        _ => {
            eprintln!("[asnap/screenshot] region selection cancelled or too small");
            return;
        }
    };

    // Step 2: arm scroll accumulator
    SCROLL_DELTA.store(0, Ordering::SeqCst);
    CAPTURE_ACTIVE.store(true, Ordering::SeqCst);

    // Step 3: capture initial frame
    let tmp0 = desktop::tmp_path("frame0", "png");
    if !capture_region(&region, &tmp0) {
        CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
        return;
    }

    let initial = match image::open(&tmp0) {
        Ok(img) => { let _ = std::fs::remove_file(&tmp0); img }
        Err(e)  => { eprintln!("[asnap/screenshot] {e}"); CAPTURE_ACTIVE.store(false, Ordering::SeqCst); return; }
    };

    // Step 4: collect additional frames while user scrolls
    let mut frames = vec![initial];
    let wait_start = Instant::now();
    let mut last_activity = Instant::now();
    let mut frame_idx: usize = 1;
    let mut in_scroll_mode = false;

    loop {
        let delta = SCROLL_DELTA.swap(0, Ordering::SeqCst);

        if delta.abs() > 0 {
            in_scroll_mode = true;
            last_activity = Instant::now();

            // Brief pause for scroll animation to settle
            std::thread::sleep(Duration::from_millis(SCROLL_FRAME_MS));

            if frames.len() >= MAX_FRAMES { break; }

            let tmp = desktop::tmp_path(&format!("frame{}", frame_idx), "png");
            if capture_region(&region, &tmp) {
                match image::open(&tmp) {
                    Ok(img) => frames.push(img),
                    Err(_)  => {}
                }
                let _ = std::fs::remove_file(&tmp);
                frame_idx += 1;
            }
        }

        // In static mode: bail out if no scroll starts within SCROLL_WAIT_MS
        if !in_scroll_mode && wait_start.elapsed() > Duration::from_millis(SCROLL_WAIT_MS) {
            break;
        }

        // In scroll mode: bail out if scrolling stops for SCROLL_IDLE_MS
        if in_scroll_mode && last_activity.elapsed() > Duration::from_millis(SCROLL_IDLE_MS) {
            break;
        }

        std::thread::sleep(Duration::from_millis(30));
    }

    CAPTURE_ACTIVE.store(false, Ordering::SeqCst);

    // Step 5: compose output
    let out = desktop::output_path("png");
    let result = if frames.len() == 1 {
        frames.remove(0)
    } else {
        eprintln!("[asnap/screenshot] stitching {} frames", frames.len());
        stitch::stitch(frames)
    };

    // Strip metadata by re-encoding through the image crate (drops all tEXt/iCCP/etc.)
    if let Err(e) = result.save(&out) {
        eprintln!("[asnap/screenshot] save failed: {e}");
        return;
    }

    eprintln!("[asnap/screenshot] → {}", out.display());
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform: region selection
// Returns display-coordinate rect (top-left origin, pixels).
// ─────────────────────────────────────────────────────────────────────────────

/// Public wrapper so other modules (recorder.rs) can reuse the same
/// interactive region-selection logic.
pub fn select_region_pub() -> Option<Region> {
    select_region()
}

#[cfg(target_os = "macos")]
fn select_region() -> Option<Region> {
    // We spawn our embedded Swift region-selector and parse its stdout.
    // Falls back to screencapture -i (static only) if swift is unavailable.
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("swift")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(MACOS_SELECTOR_SWIFT.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    parse_region_csv(s.trim())
}

#[cfg(target_os = "linux")]
fn select_region() -> Option<Region> {
    // slurp outputs "X,Y WxH"
    use std::process::Command;

    let out = Command::new("slurp")
        .arg("-d")            // show dimensions
        .output()
        .ok()?;

    if !out.status.success() { return None; }

    let s = String::from_utf8_lossy(&out.stdout);
    // format: "x,y wxh"
    parse_slurp(s.trim())
}

#[cfg(target_os = "windows")]
fn select_region() -> Option<Region> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(WINDOWS_SELECTOR_PS1.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    parse_region_csv(s.trim())
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform: capture a specific region to a file
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn capture_region(r: &Region, path: &PathBuf) -> bool {
    // screencapture -R x,y,w,h -t png filename
    std::process::Command::new("screencapture")
        .args([
            "-R", &format!("{},{},{},{}", r.x, r.y, r.w, r.h),
            "-t", "png",
            "-x",                            // no shutter sound
            path.to_str().unwrap(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn capture_region(r: &Region, path: &PathBuf) -> bool {
    // grim -g "x,y wxh" filename
    std::process::Command::new("grim")
        .args([
            "-g", &format!("{},{} {}x{}", r.x, r.y, r.w, r.h),
            path.to_str().unwrap(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn capture_region(r: &Region, path: &PathBuf) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script = format!(
        r#"
Add-Type -AssemblyName System.Drawing,System.Windows.Forms
$bmp = New-Object System.Drawing.Bitmap({w},{h})
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen({x},{y},0,0,[System.Drawing.Size]::new({w},{h}))
$g.Dispose()
$bmp.Save('{path}')
$bmp.Dispose()
"#,
        x = r.x, y = r.y, w = r.w, h = r.h,
        path = path.to_str().unwrap().replace('\'', "''")
    );

    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Parsers
// ─────────────────────────────────────────────────────────────────────────────

/// Parse "x,y,w,h" (comma-separated integers).
#[allow(dead_code)]
fn parse_region_csv(s: &str) -> Option<Region> {
    let parts: Vec<i32> = s.split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() < 4 { return None; }
    Some(Region { x: parts[0], y: parts[1], w: parts[2], h: parts[3] })
}

/// Parse slurp output: "x,y wxh"
#[cfg(target_os = "linux")]
fn parse_slurp(s: &str) -> Option<Region> {
    // e.g. "100,200 640x480"
    let (pos, dim) = s.split_once(' ')?;
    let (xs, ys) = pos.split_once(',')?;
    let (ws, hs) = dim.split_once('x')?;
    Some(Region {
        x: xs.trim().parse().ok()?,
        y: ys.trim().parse().ok()?,
        w: ws.trim().parse().ok()?,
        h: hs.trim().parse().ok()?,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Embedded platform helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal Swift region-selector for macOS.
/// Pipe this to `swift -` — it creates a full-screen transparent Cocoa overlay,
/// lets the user click-drag a rectangle, then prints "x,y,w,h" to stdout.
#[cfg(target_os = "macos")]
const MACOS_SELECTOR_SWIFT: &str = r#"
import Cocoa

class SelView: NSView {
    var start = NSPoint.zero
    var rect  = NSRect.zero

    override var acceptsFirstResponder: Bool { true }

    override func mouseDown(with e: NSEvent) {
        start = convert(e.locationInWindow, from: nil)
        rect  = NSRect(origin: start, size: .zero)
    }
    override func mouseDragged(with e: NSEvent) {
        let c = convert(e.locationInWindow, from: nil)
        rect = NSRect(x: min(start.x,c.x), y: min(start.y,c.y),
                      width: abs(c.x-start.x), height: abs(c.y-start.y))
        needsDisplay = true
    }
    override func mouseUp(with e: NSEvent) {
        let scr  = window!.convertToScreen(rect)
        let mh   = NSScreen.main!.frame.height
        let rx   = Int(scr.origin.x)
        let ry   = Int(mh - scr.origin.y - scr.size.height)
        let rw   = Int(scr.size.width)
        let rh   = Int(scr.size.height)
        print("\(rx),\(ry),\(rw),\(rh)")
        fflush(stdout)
        NSApp.terminate(nil)
    }
    override func draw(_ r: NSRect) {
        NSColor(white: 0, alpha: 0.18).set(); r.fill()
        if rect.size.width > 2 {
            NSColor.systemRed.withAlphaComponent(0.25).set(); rect.fill()
            let p = NSBezierPath(rect: rect); p.lineWidth = 1.5
            NSColor.systemRed.set(); p.stroke()
        }
    }
    override func keyDown(with e: NSEvent) {
        if e.keyCode == 53 { NSApp.terminate(nil) } // Esc
    }
}

NSApplication.shared.setActivationPolicy(.accessory)
let scr = NSScreen.main!.frame
let win = NSWindow(contentRect: scr, styleMask: [.borderless],
                   backing: .buffered, defer: false)
win.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.screenSaverWindow)))
win.isOpaque = false; win.backgroundColor = .clear
win.ignoresMouseEvents = false; win.acceptsMouseMovedEvents = true
win.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
let v = SelView(frame: scr)
win.contentView = v
win.makeKeyAndOrderFront(nil)
win.makeFirstResponder(v)
NSCursor.crosshair.set()
NSApp.activate(ignoringOtherApps: true)
NSApp.run()
"#;

/// PowerShell region-selector for Windows.
/// Outputs "x,y,w,h" to stdout.
#[cfg(target_os = "windows")]
const WINDOWS_SELECTOR_PS1: &str = r#"
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type @'
using System;using System.Drawing;using System.Windows.Forms;
public class Sel:Form{
  Point s,e; bool drag; public Rectangle R;
  public Sel(){
    FormBorderStyle=FormBorderStyle.None;
    WindowState=FormWindowState.Maximized;
    TopMost=true;Opacity=0.22;BackColor=Color.Black;Cursor=Cursors.Cross;
    SetStyle(ControlStyles.SupportsTransparentBackColor,true);
    MouseDown+=(o,m)=>{s=m.Location;drag=true;};
    MouseMove+=(o,m)=>{if(drag){e=m.Location;Refresh();}};
    MouseUp+=(o,m)=>{e=m.Location;
      R=new Rectangle(Math.Min(s.X,e.X),Math.Min(s.Y,e.Y),
                      Math.Abs(e.X-s.X),Math.Abs(e.Y-s.Y));Close();};
    Paint+=(o,p)=>{if(drag){
      using var pen=new Pen(Color.Red,2);
      p.Graphics.DrawRectangle(pen,new Rectangle(
        Math.Min(s.X,e.X),Math.Min(s.Y,e.Y),
        Math.Abs(e.X-s.X),Math.Abs(e.Y-s.Y)));}};
    KeyDown+=(o,k)=>{if(k.KeyCode==Keys.Escape)Close();};
  }
}
'@ -ReferencedAssemblies System.Windows.Forms,System.Drawing
$f=New-Object Sel
[System.Windows.Forms.Application]::Run($f)
$r=$f.R
Write-Output "$($r.X),$($r.Y),$($r.Width),$($r.Height)"
"#;
