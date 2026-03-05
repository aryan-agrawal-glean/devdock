//! Pure-Rust extraction of IAP cookies from the local Chrome cookie store.
//!
//! Best-effort: any failure (missing browser, locked DB, decryption error) is
//! silently swallowed and the function returns `None`.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info, warn};

/// A single IAP cookie extracted from a browser.
#[derive(Debug, Serialize)]
struct IapCookie {
    name: String,
    value: String,
    domain: String,
}

/// Attempt to read IAP cookies from the local Chrome cookie store.
///
/// Looks for `__Host-GCP_IAP_AUTH_TOKEN_*` cookies scoped to `*.glean.com`.
/// Returns `Some(json_string)` with a JSON array of `{name, value, domain}` objects,
/// or `None` if no cookies are found or any error occurs.
pub fn read_local_iap_cookies(verbose: bool) -> Option<String> {
    debug!("Attempting to read IAP cookies from local Chrome");

    let cookies = match read_chrome_cookies() {
        Ok(c) => c,
        Err(e) => {
            debug!("Chrome cookie extraction failed: {e}");
            Vec::new()
        }
    };

    if cookies.is_empty() {
        debug!("No IAP cookies found in local browser");
        if verbose {
            println!("    → No IAP cookies found in local browser (Erasmus may need manual login)");
        }
        return None;
    }

    // Build the format Erasmus expects:
    // {"domain": "apps-gke.glean.com", "cookies": {"cookie_name": "value", ...}}
    let mut cookie_map = serde_json::Map::new();
    for c in &cookies {
        cookie_map.insert(c.name.clone(), serde_json::Value::String(c.value.clone()));
    }
    let cache_data = serde_json::json!({
        "domain": "apps-gke.glean.com",
        "cookies": cookie_map,
    });

    match serde_json::to_string(&cache_data) {
        Ok(json) => {
            info!("Found {} IAP cookie(s) for Erasmus", cookies.len());
            if verbose {
                println!("    → Found IAP cookies for Erasmus remote sync");
            }
            Some(json)
        }
        Err(e) => {
            warn!("Failed to serialize IAP cookies: {e}");
            None
        }
    }
}

// ─── Chrome ────────────────────────────────────────────────────────────────────

/// Locate the Chrome Cookies SQLite database.
fn chrome_cookie_db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    #[cfg(target_os = "macos")]
    let candidates = vec![
        home.join("Library/Application Support/Google/Chrome/Default/Cookies"),
        home.join("Library/Application Support/Google/Chrome/Profile 1/Cookies"),
    ];

    #[cfg(target_os = "linux")]
    let candidates = vec![
        home.join(".config/google-chrome/Default/Cookies"),
        home.join(".config/google-chrome/Profile 1/Cookies"),
        home.join(".config/chromium/Default/Cookies"),
    ];

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let candidates: Vec<PathBuf> = vec![];

    candidates.into_iter().find(|p| p.exists())
}

/// Read IAP cookies from Chrome's cookie database.
fn read_chrome_cookies() -> Result<Vec<IapCookie>, Box<dyn std::error::Error>> {
    let db_path = chrome_cookie_db_path().ok_or("Chrome cookie DB not found")?;
    debug!("Reading Chrome cookies from {}", db_path.display());

    // Copy the DB to a temp file to avoid locking issues with a running browser.
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::copy(&db_path, tmp.path())?;

    let conn = rusqlite::Connection::open_with_flags(
        tmp.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stmt = conn.prepare(
        "SELECT name, encrypted_value, host_key FROM cookies \
         WHERE host_key LIKE '%.glean.com' \
         AND name LIKE '__Host-GCP_IAP_AUTH_TOKEN%'",
    )?;

    let decryption_key = chrome_decryption_key();

    let mut cookies = Vec::new();
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let encrypted_value: Vec<u8> = row.get(1)?;
        let domain: String = row.get(2)?;
        Ok((name, encrypted_value, domain))
    })?;

    for row in rows {
        let (name, encrypted_value, domain) = row?;

        let value = if encrypted_value.is_empty() {
            // Very old Chrome versions stored plaintext – unlikely but handle it.
            continue;
        } else if let Some(ref key) = decryption_key {
            match decrypt_chrome_cookie(&encrypted_value, key) {
                Ok(v) => v,
                Err(e) => {
                    debug!("Failed to decrypt Chrome cookie {name}: {e}");
                    continue;
                }
            }
        } else {
            debug!("No Chrome decryption key available, skipping {name}");
            continue;
        };

        if !value.is_empty() {
            cookies.push(IapCookie { name, value, domain });
        }
    }

    Ok(cookies)
}

/// Derive the AES-128-CBC key Chrome uses to encrypt cookie values.
///
/// On macOS the password lives in the Keychain ("Chrome Safe Storage"),
/// on Linux it comes from the Secret Service or falls back to `"peanuts"`.
fn chrome_decryption_key() -> Option<Vec<u8>> {
    let password = chrome_password()?;

    let salt = b"saltysalt";
    #[cfg(target_os = "macos")]
    let iterations = 1003;
    #[cfg(not(target_os = "macos"))]
    let iterations = 1;

    let mut key = vec![0u8; 16];
    openssl::pkcs5::pbkdf2_hmac(
        password.as_bytes(),
        salt,
        iterations,
        openssl::hash::MessageDigest::sha1(),
        &mut key,
    )
    .ok()?;

    Some(key)
}

/// Retrieve Chrome's encryption password from the OS credential store.
fn chrome_password() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("security")
            .args(["find-generic-password", "-s", "Chrome Safe Storage", "-w"])
            .output()
            .ok()?;
        if output.status.success() {
            let pw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !pw.is_empty() {
                return Some(pw);
            }
        }
        None
    }

    #[cfg(target_os = "linux")]
    {
        // Try GNOME Keyring / Secret Service via secret-tool
        if let Ok(output) = Command::new("secret-tool")
            .args(["lookup", "application", "chrome"])
            .output()
        {
            if output.status.success() {
                let pw = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !pw.is_empty() {
                    return Some(pw);
                }
            }
        }

        // Fallback: Chromium uses "peanuts" when no keyring is available.
        Some("peanuts".to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Decrypt a single Chrome encrypted cookie value.
///
/// Chrome prefixes the ciphertext with a 3-byte version tag:
///   `v10` on macOS, `v11` (or `v10`) on Linux.
/// The remainder is AES-128-CBC encrypted with IV = 16 × 0x20 (space).
///
/// The decrypted plaintext has a 32-byte hash prefix (integrity MAC)
/// followed by the actual cookie value.
fn decrypt_chrome_cookie(
    encrypted: &[u8],
    key: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    // Must have at least the version prefix.
    if encrypted.len() < 3 {
        return Err("encrypted value too short".into());
    }

    let version = &encrypted[..3];
    if version != b"v10" && version != b"v11" {
        return Err(format!("unknown Chrome cookie version: {:?}", version).into());
    }

    let ciphertext = &encrypted[3..];
    let iv = [b' '; 16]; // 0x20

    let cipher = openssl::symm::Cipher::aes_128_cbc();
    let plaintext = openssl::symm::decrypt(cipher, key, Some(&iv), ciphertext)?;

    // Chrome prepends a 32-byte hash/MAC to the plaintext cookie value.
    const HASH_PREFIX_LEN: usize = 32;
    if plaintext.len() <= HASH_PREFIX_LEN {
        return Err("decrypted value too short after stripping hash prefix".into());
    }
    let value = &plaintext[HASH_PREFIX_LEN..];

    String::from_utf8(value.to_vec())
        .map_err(|e| format!("decrypted cookie is not valid UTF-8: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypt_chrome_cookie_rejects_short_input() {
        let key = vec![0u8; 16];
        assert!(decrypt_chrome_cookie(b"v1", &key).is_err());
    }

    #[test]
    fn test_decrypt_chrome_cookie_rejects_unknown_version() {
        let key = vec![0u8; 16];
        assert!(decrypt_chrome_cookie(b"v99some_data", &key).is_err());
    }

    #[test]
    fn test_iap_cookie_serialization() {
        let cookies = vec![IapCookie {
            name: "__Host-GCP_IAP_AUTH_TOKEN_123".to_string(),
            value: "test_value".to_string(),
            domain: ".glean.com".to_string(),
        }];
        let json = serde_json::to_string(&cookies).unwrap();
        assert!(json.contains("__Host-GCP_IAP_AUTH_TOKEN_123"));
        assert!(json.contains("test_value"));
        assert!(json.contains(".glean.com"));
    }
}
