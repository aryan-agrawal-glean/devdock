use std::process::Command;

fn main() {
    let profile = std::env::var("PROFILE").unwrap_or_default();

    let suffix = if profile == "release" {
        String::new()
    } else {
        // Get git SHA
        let sha = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check if working tree is dirty
        let dirty = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);

        if dirty {
            format!("-{sha}-dirty")
        } else {
            format!("-{sha}")
        }
    };

    println!("cargo:rustc-env=DEV_VERSION_SUFFIX={suffix}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
