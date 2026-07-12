// ocr.rs — Alt + \
//
// Flow:
//   1. Select region (reuse screenshot's selector).
//   2. Capture region to a temp PNG.
//   3. Run platform-native OCR engine on that PNG → text.
//   4. Write text to clipboard.
//   5. Write text to Desktop/<timestamp>.txt as offline backup.

use crate::desktop;
use crate::screenshot::select_region_pub;

pub fn run() {
    let region = match select_region_pub() {
        Some(r) if r.w > 2 && r.h > 2 => r,
        _ => {
            eprintln!("[asnap/ocr] region selection cancelled or too small");
            return;
        }
    };

    let tmp = desktop::tmp_path("ocr", "png");
    if !capture_region(&region, &tmp) {
        eprintln!("[asnap/ocr] capture failed");
        return;
    }

    let text = recognize(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);

    if text.trim().is_empty() {
        eprintln!("[asnap/ocr] no text recognized");
        return;
    }

    // Write to clipboard
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.clone());
    } else {
        eprintln!("[asnap/ocr] clipboard unavailable");
    }

    // Write .txt backup to Desktop
    let out = desktop::output_path("txt");
    if let Err(e) = std::fs::write(&out, &text) {
        eprintln!("[asnap/ocr] failed to write {}: {e}", out.display());
        return;
    }

    eprintln!("[asnap/ocr] → clipboard + {}", out.display());
}

// ─────────────────────────────────────────────────────────────────────────────
// Capture (shared logic, duplicated minimal version to avoid cross-module
// visibility friction — keeps modules independently testable)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn capture_region(r: &crate::screenshot::Region, path: &std::path::PathBuf) -> bool {
    std::process::Command::new("screencapture")
        .args([
            "-R", &format!("{},{},{},{}", r.x, r.y, r.w, r.h),
            "-t", "png", "-x",
            path.to_str().unwrap(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn capture_region(r: &crate::screenshot::Region, path: &std::path::PathBuf) -> bool {
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
fn capture_region(r: &crate::screenshot::Region, path: &std::path::PathBuf) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let script = format!(
        r#"
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap({w},{h})
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen({x},{y},0,0,[System.Drawing.Size]::new({w},{h}))
$g.Dispose(); $bmp.Save('{path}'); $bmp.Dispose()
"#,
        x = r.x, y = r.y, w = r.w, h = r.h,
        path = path.to_str().unwrap().replace('\'', "''")
    );
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().unwrap();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform-native OCR engines
// ─────────────────────────────────────────────────────────────────────────────

/// macOS: Vision.framework via an embedded Swift one-shot script.
#[cfg(target_os = "macos")]
fn recognize(path: &std::path::PathBuf) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script = format!(
        r#"
import Vision
import AppKit

let url = URL(fileURLWithPath: "{path}")
guard let img = NSImage(contentsOf: url),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {{
    exit(1)
}}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true

let handler = VNImageRequestHandler(cgImage: cg, options: [:])
try? handler.perform([request])

guard let results = request.results else {{ exit(1) }}
for obs in results {{
    if let candidate = obs.topCandidates(1).first {{
        print(candidate.string)
    }}
}}
"#,
        path = path.to_str()?.replace('"', "\\\"")
    );

    let mut child = Command::new("swift")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Windows: Windows.Media.Ocr UWP engine via embedded PowerShell + WinRT.
#[cfg(target_os = "windows")]
fn recognize(path: &std::path::PathBuf) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script = format!(
        r#"
[Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime] | Out-Null
[Windows.Media.Ocr.OcrEngine,Windows.Foundation.UniversalApiContract,ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder,Windows.Graphics,ContentType=WindowsRuntime] | Out-Null

function Await($task, $type) {{
    $asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {{
        $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1
    }} | Select-Object -First 1
    $genMethod = $asTask.MakeGenericMethod($type)
    $netTask = $genMethod.Invoke($null, @($task))
    $netTask.Wait(-1) | Out-Null
    return $netTask.Result
}}

$path = "{path}"
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])

Write-Output $result.Text
"#,
        path = path.to_str()?.replace('\\', "\\\\")
    );

    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Linux: tesseract via process pipe (uses system tesseract if present,
/// otherwise falls back to an ephemeral `nix-shell -p tesseract`).
#[cfg(target_os = "linux")]
fn recognize(path: &std::path::PathBuf) -> Option<String> {
    use std::process::Command;

    // Try system tesseract first.
    let direct = Command::new("tesseract")
        .args([path.to_str()?, "stdout", "-l", "eng"])
        .output();

    if let Ok(out) = &direct {
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).to_string());
        }
    }

    // Fallback: ephemeral nix-shell
    let out = Command::new("nix-shell")
        .args([
            "-p", "tesseract",
            "--run",
            &format!("tesseract '{}' stdout -l eng", path.to_str()?),
        ])
        .output()
        .ok()?;

    Some(String::from_utf8_lossy(&out.stdout).to_string())
}
