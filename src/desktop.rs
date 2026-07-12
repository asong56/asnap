// desktop.rs — resolve Desktop path and generate timestamped filenames.

use std::path::PathBuf;
use chrono::Local;

/// Returns the user Desktop directory, falling back to ~/Desktop.
pub fn dir() -> PathBuf {
    dirs::desktop_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .expect("cannot locate home directory")
            .join("Desktop")
    })
}

/// Returns a full Desktop path like `~/Desktop/260625_150520.png`.
/// The timestamp format is `YYMMDD_HHMMSS`.
pub fn output_path(ext: &str) -> PathBuf {
    let ts = Local::now().format("%y%m%d_%H%M%S").to_string();
    dir().join(format!("{}.{}", ts, ext))
}

/// Returns a temporary file path in the system temp dir.
pub fn tmp_path(tag: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!("asnap_{}_{}.{}", tag, std::process::id(), ext))
}
