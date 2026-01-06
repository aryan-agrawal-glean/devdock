//! Output formatting utilities.

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Print a success message.
pub fn success(msg: &str) {
    println!("{} {}", "✓".green().bold(), msg);
}

/// Print an info message.
pub fn info(msg: &str) {
    println!("{} {}", "⏩".blue().bold(), msg);
}

/// Print a warning message.
pub fn warn(msg: &str) {
    println!("{} {}", "⚠️".yellow().bold(), msg);
}

/// Print an error message.
#[allow(dead_code)]
pub fn error(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg);
}

/// Print a section header.
pub fn header(msg: &str) {
    let line = "═".repeat(60);
    println!();
    println!("{}", line.blue());
    println!("{}", format!("  {}", msg).bold());
    println!("{}", line.blue());
    println!();
}

/// Print a step in a multi-step process.
pub fn step(current: usize, total: usize, msg: &str) {
    println!(
        "{} {} {}",
        format!("[{}/{}]", current, total).purple().bold(),
        "▶".cyan(),
        msg
    );
}

/// Create a progress bar with time estimate.
#[allow(dead_code)]
pub fn create_progress_bar(total_seconds: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total_seconds);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40.cyan/blue}] {pos}/{len}s (elapsed: {elapsed})")
            .unwrap()
            .progress_chars("█▓░"),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Create a spinner for indefinite operations.
pub fn create_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Format elapsed time as human-readable string.
pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// Send desktop notification (macOS-specific for now).
#[cfg(target_os = "macos")]
pub fn notify(title: &str, message: &str) {
    use std::process::Command;
    let _ = Command::new("osascript")
        .args(&[
            "-e",
            &format!(
                r#"display notification "{}" with title "{}" sound name "Glass""#,
                message, title
            ),
        ])
        .output();
}

/// Send desktop notification (Linux-specific).
#[cfg(target_os = "linux")]
pub fn notify(title: &str, message: &str) {
    use std::process::Command;
    let _ = Command::new("notify-send")
        .args(&[title, message])
        .output();
}

/// Send desktop notification (Windows - no-op for now).
#[cfg(target_os = "windows")]
pub fn notify(_title: &str, _message: &str) {
    // Windows notification support can be added later
}

