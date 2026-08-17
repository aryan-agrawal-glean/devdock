//! Canonical paths for legacy and versioned DevDock state.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Owns every path introduced by the versioned instance-state layout.
#[derive(Debug, Clone)]
pub struct StatePaths {
    root: PathBuf,
}

impl StatePaths {
    pub fn from_home(home: &Path) -> Self {
        Self { root: home.join(".devdock") }
    }

    pub fn from_database(db_path: &Path) -> Result<Self> {
        let parent =
            db_path.parent().context("state database path has no parent directory")?.to_path_buf();
        let root = if db_path.file_name().is_some_and(|name| name == "devdock.db") {
            parent
        } else {
            let file_name = db_path
                .file_name()
                .context("state database path has no file name")?
                .to_string_lossy();
            parent.join(format!(".{file_name}-devdock-state"))
        };
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn database(&self) -> PathBuf {
        self.root.join("devdock.db")
    }

    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance(&self, instance_id: &str) -> Result<PathBuf> {
        validate_component(instance_id, "instance ID")?;
        Ok(self.instances().join(instance_id))
    }

    pub fn session_manifest(&self, instance_id: &str) -> Result<PathBuf> {
        Ok(self.instance(instance_id)?.join("session.json"))
    }

    pub fn ensure_root(&self) -> Result<()> {
        create_private_dir(&self.root)
    }
}

pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create private state directory {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to set private permissions on {}", path.display()))?;
    }

    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("invalid {label}: expected only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_paths_reject_traversal() {
        let paths = StatePaths::from_home(Path::new("/tmp/home"));
        assert!(paths.instance("../outside").is_err());
        assert!(paths.instance("dev-valid_123").is_ok());
    }
}
