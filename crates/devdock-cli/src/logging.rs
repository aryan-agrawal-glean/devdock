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
        
        let timestamp = Local::now().format("%Y%m%d-%H%M%S");
        let log_path = log_dir.join(format!("devdock-remote-{}.log", timestamp));
        
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        
        let logger = Self {
            file: Arc::new(Mutex::new(file)),
            log_path,
        };
        
        logger.log_line(&format!("=== DevDock Remote Session Started ==="))?;
        logger.log_line(&format!("DevDock ID: {}", devdock_id))?;
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
        writeln!(file, "[{}] {}", timestamp, message)?;
        file.flush()?;
        Ok(())
    }
    
    /// Log a message without timestamp.
    pub fn log_line(&self, message: &str) -> Result<()> {
        let mut file = self.file.lock().unwrap();
        writeln!(file, "{}", message)?;
        file.flush()?;
        Ok(())
    }
    
    /// Log a section header.
    pub fn log_section(&self, title: &str) -> Result<()> {
        self.log_line("")?;
        self.log_line(&format!("=== {} ===", title))?;
        Ok(())
    }
    
    /// Log an error.
    pub fn log_error(&self, error: &str) -> Result<()> {
        self.log_section("ERROR")?;
        self.log(error)?;
        Ok(())
    }
    
    /// Log command execution.
    pub fn log_command(&self, command: &str) -> Result<()> {
        self.log(&format!("Executing: {}", command))?;
        Ok(())
    }
}

impl Clone for OperationLogger {
    fn clone(&self) -> Self {
        Self {
            file: Arc::clone(&self.file),
            log_path: self.log_path.clone(),
        }
    }
}

