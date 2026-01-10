//! Archive creation for bug reports.

use crate::{system::SystemInfo, Result};
use chrono::Local;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;
use zip::write::FileOptions;
use zip::ZipWriter;

/// Create a ZIP archive with all bug report data.
pub fn create_report_archive(
    system_info: &SystemInfo,
    snapshot_file: Option<&PathBuf>,
    config_files: &HashMap<String, String>,
) -> Result<PathBuf> {
    // Create temp directory for archive
    let temp_dir =
        std::env::temp_dir().join(format!("devdock_bug_report_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    // Create archive filename
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let archive_name = format!("devdock_bug_report_{timestamp}.zip");
    let archive_path = temp_dir.join(&archive_name);

    info!("Creating bug report archive: {:?}", archive_path);

    let file = File::create(&archive_path)?;
    let mut zip = ZipWriter::new(file);

    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    // Add system_info.json
    zip.start_file("system_info.json", options)?;
    let system_info_json = serde_json::to_string_pretty(system_info)?;
    zip.write_all(system_info_json.as_bytes())?;

    // Add Tilt snapshot if available
    if let Some(snapshot_path) = snapshot_file {
        if snapshot_path.exists() {
            zip.start_file("tilt_state.json", options)?;
            let snapshot_content = fs::read(snapshot_path)?;
            zip.write_all(&snapshot_content)?;
        }
    }

    // Add configuration files
    for (filename, content) in config_files {
        let safe_filename = filename.replace('/', "_");
        zip.start_file(&safe_filename, options)?;
        zip.write_all(content.as_bytes())?;
    }

    zip.finish()?;

    info!("Created archive: {:?} ({} bytes)", archive_path, fs::metadata(&archive_path)?.len());

    Ok(archive_path)
}

/// Clean up temporary files and directories.
pub fn cleanup_temp_files(files: &[PathBuf]) {
    for file_path in files {
        if let Err(e) = remove_path(file_path) {
            tracing::warn!("Failed to clean up {:?}: {}", file_path, e);
        }
    }
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    if path.is_file() {
        fs::remove_file(path)
    } else if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        Ok(())
    }
}
