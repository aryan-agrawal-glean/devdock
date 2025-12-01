//! Output formatting utilities.

use colored::Colorize;

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

