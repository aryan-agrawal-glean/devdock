//! Cross-platform desktop notification support.

use std::process::Command;
use tracing::{debug, warn};

/// Notification service for sending desktop alerts.
#[derive(Debug, Clone, Default)]
pub struct Notifier {
    enabled: bool,
}

impl Notifier {
    pub fn new() -> Self {
        Notifier { enabled: true }
    }

    pub fn with_enabled(enabled: bool) -> Self {
        Notifier { enabled }
    }

    /// Send a desktop notification.
    pub fn send(&self, title: &str, message: &str) {
        if !self.enabled {
            return;
        }

        debug!("Sending notification: {} - {}", title, message);

        #[cfg(target_os = "macos")]
        self.send_macos(title, message);

        #[cfg(target_os = "linux")]
        self.send_linux(title, message);

        #[cfg(target_os = "windows")]
        self.send_windows(title, message);
    }

    /// Send success notification.
    pub fn success(&self, message: &str) {
        self.send("DevDock Startup", message);
    }

    /// Send failure notification.
    pub fn failure(&self, message: &str) {
        self.send("DevDock Startup Failed", message);
    }

    #[cfg(target_os = "macos")]
    fn send_macos(&self, title: &str, message: &str) {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            message.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );

        if let Err(e) = Command::new("osascript").args(["-e", &script]).output() {
            warn!("Failed to send macOS notification: {}", e);
        }
    }

    #[cfg(target_os = "linux")]
    fn send_linux(&self, title: &str, message: &str) {
        if let Err(e) = Command::new("notify-send").args([title, message]).output() {
            warn!("Failed to send Linux notification: {}", e);
        }
    }

    #[cfg(target_os = "windows")]
    fn send_windows(&self, title: &str, message: &str) {
        let script = format!(
            r#"
            [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
            [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
            $xml = @"
            <toast>
                <visual>
                    <binding template='ToastGeneric'>
                        <text>{}</text>
                        <text>{}</text>
                    </binding>
                </visual>
            </toast>
"@
            $XmlDocument = [Windows.Data.Xml.Dom.XmlDocument]::new()
            $XmlDocument.LoadXml($xml)
            $toast = [Windows.UI.Notifications.ToastNotification]::new($XmlDocument)
            [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("DevDock").Show($toast)
            "#,
            title, message
        );

        if let Err(e) = Command::new("powershell")
            .args(["-Command", &script])
            .output()
        {
            warn!("Failed to send Windows notification: {}", e);
        }
    }
}

