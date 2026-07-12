*I haven't tested it on macos and linux, welcome everyone's contribution. thx*

# asnap

Zero-UI, hotkey-driven, single-binary screen-capture daemon. No config file,
no tray icon, no window. Press a hotkey, get a file on your Desktop.

## Hotkeys

| Hotkey     | Action                                                            |
|------------|--------------------------------------------------------------------|
| `Alt + [`  | Smart screenshot. Drag a region and release → `.png`. Drag, then **scroll down without releasing**, then release → stitched long screenshot `.png`. |
| `Alt + ]`  | Toggle region recording. First press selects a region and starts recording; second press stops and muxes the `.mp4`. |
| `Alt + \`  | Region OCR. Recognized text is pushed to the clipboard **and** saved as a `.txt` backup. |

All output lands directly on the Desktop, named `YYMMDD_HHMMSS.<ext>` — e.g.
`260625_150520.png`.

## Build

Requires Rust (`rustup`) and platform build tools.

```bash
# Linux (Hyprland/Wayland) — needs grim + slurp, and an mp4 recorder
cargo build --release --target x86_64-unknown-linux-gnu

# macOS Apple Silicon
cargo build --release --target aarch64-apple-darwin

# Windows x64 (static CRT, see .cargo/config.toml)
cargo build --release --target x86_64-pc-windows-msvc
```

The result is a single native executable in `target/<triple>/release/`.

## Runtime dependencies (already on the OS, nothing bundled)

- **Windows**: PowerShell + .NET (`System.Drawing`/`System.Windows.Forms`) for
  the selector and capture; `Windows.Media.Ocr` (UWP) for OCR; `ffmpeg.exe`
  on `PATH` for recording (`gdigrab`).
- **macOS**: `screencapture` (capture/record), `swift` (selector + Vision.framework
  OCR) — both ship with Xcode Command Line Tools.
- **Linux (Hyprland/Wayland)**: `slurp` + `grim` (selection/capture),
  `wl-screenrec` or `wf-recorder` (recording, VA-API), `tesseract` (OCR — or
  it's pulled ephemerally via `nix-shell -p tesseract` if Nix is available
  and the binary isn't on `PATH`).

## Design notes

- **Long screenshot**: while the selection rectangle is held, scroll-wheel
  events are accumulated (`SCROLL_DELTA` atomic). Frames are captured at
  ~220 ms intervals while scrolling continues, then stitched by detecting the
  row-overlap between consecutive frames (sampled MSE search) and
  concatenating only the unique remainder of each new frame.
- **Privacy**: all PNG/MP4/TXT output is generated fresh through the `image`
  crate / native OS encoders rather than copying raw OS capture buffers, so
  EXIF/ICC/GPS metadata never round-trips into the saved file.
- **Footprint**: release profile uses `opt-level = "z"`, LTO, single codegen
  unit, and `strip = true` to keep the binary minimal; no config files or
  caches are ever written — only the Desktop output files and a transient
  temp file per capture (deleted immediately after use).

## Permissions

- **macOS**: grant Accessibility (for global hotkeys) and Screen Recording
  permission on first run.
- **Linux**: the invoking user must be able to read `/dev/input/event*`
  (typically the `input` group) for `rdev`'s global key listener to work
  under Wayland.
- **Windows**: no special permissions needed; some antivirus heuristics may
  flag a no-UI background hotkey listener — code-sign for distribution.
