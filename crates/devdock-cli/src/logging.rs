//! Logging utilities for remote DevDock operations.

use anyhow::Result;
use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Logger for remote DevDock operations.
pub struct OperationLogger {
    file: Arc<Mutex<File>>,
    log_path: PathBuf,
}

impl OperationLogger {
    /// Create a new logger for a DevDock instance.
    pub fn new(devdock_id: &str) -> Result<Self> {
        let log_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".devdock")
            .join("remote");

        std::fs::create_dir_all(&log_dir)?;

        let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let log_path = log_dir.join(operation_log_file_name(&timestamp, devdock_id));

        let file = OpenOptions::new().create(true).append(true).open(&log_path)?;

        let logger = Self { file: Arc::new(Mutex::new(file)), log_path };

        logger.log_line("=== DevDock Remote Session Started ===")?;
        logger.log_line(&format!("DevDock ID: {devdock_id}"))?;
        logger.log_line(&format!("Timestamp: {}", Local::now().to_rfc3339()))?;
        logger.log_line("")?;

        Ok(logger)
    }

    /// Get the path to the log file.
    pub fn path(&self) -> &Path {
        &self.log_path
    }

    /// Log a message with timestamp.
    pub fn log(&self, message: &str) -> Result<()> {
        let timestamp = Local::now().format("%H:%M:%S");
        let mut file = self.file.lock().unwrap();
        writeln!(file, "[{timestamp}] {message}")?;
        file.flush()?;
        Ok(())
    }

    /// Log a message without timestamp.
    pub fn log_line(&self, message: &str) -> Result<()> {
        let mut file = self.file.lock().unwrap();
        writeln!(file, "{message}")?;
        file.flush()?;
        Ok(())
    }

    /// Log a section header.
    pub fn log_section(&self, title: &str) -> Result<()> {
        self.log_line("")?;
        self.log_line(&format!("=== {title} ==="))?;
        Ok(())
    }

    /// Log an error.
    #[allow(dead_code)]
    pub fn log_error(&self, error: &str) -> Result<()> {
        self.log_section("ERROR")?;
        self.log(error)?;
        Ok(())
    }

    /// Log command execution.
    #[allow(dead_code)]
    pub fn log_command(&self, command: &str) -> Result<()> {
        self.log(&format!("Executing: {command}"))?;
        Ok(())
    }
}

fn operation_log_file_name(timestamp: &str, devdock_id: &str) -> String {
    let safe_id = devdock_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("devdock-remote-{timestamp}-{safe_id}.log")
}

impl Clone for OperationLogger {
    fn clone(&self) -> Self {
        Self { file: Arc::clone(&self.file), log_path: self.log_path.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_log_filename_is_correlated_to_one_safe_session_id() {
        let first = operation_log_file_name("20260910-120000", "dev-1111");
        let second = operation_log_file_name("20260910-120000", "dev-2222");

        assert_eq!(first, "devdock-remote-20260910-120000-dev-1111.log");
        assert_ne!(first, second);
        assert_eq!(
            operation_log_file_name("20260910-120000", "dev/unsafe value"),
            "devdock-remote-20260910-120000-dev-unsafe-value.log"
        );
    }
}
