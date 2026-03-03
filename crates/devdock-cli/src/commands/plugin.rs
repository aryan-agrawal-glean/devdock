//! Plugin management commands.

use crate::output;
use anyhow::Result;

pub async fn list() -> Result<()> {
    output::info("Registered backends:");
    println!("  Telemetry: stdout (default), bigquery (--features gcp)");
    println!("  Storage:   local (default), gcs (--features gcp)");
    println!("  Remote:    ssh (default), coder");
    println!("  Diagnostics: stdout (default), jira");
    println!();
    println!("  External plugins: ~/.devdock/plugins/");

    let plugins_dir = dirs::home_dir().unwrap_or_default().join(".devdock/plugins");
    if plugins_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                println!("    - {name}");
            }
        }
    } else {
        println!("    (none installed)");
    }

    Ok(())
}

pub async fn install(_name: &str) -> Result<()> {
    output::warn("Plugin install not yet implemented - coming soon!");
    Ok(())
}

pub async fn remove(_name: &str) -> Result<()> {
    output::warn("Plugin remove not yet implemented - coming soon!");
    Ok(())
}

/// Create a new external plugin from a template.
pub async fn create(name: &str, plugin_type: &str, language: &str) -> Result<()> {
    let plugins_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".devdock/plugins")
        .join(name);

    if plugins_dir.exists() {
        anyhow::bail!("Plugin '{}' already exists at {}", name, plugins_dir.display());
    }

    std::fs::create_dir_all(&plugins_dir)?;

    // Generate plugin metadata
    let metadata = format!(
        r#"[plugin]
name = "{name}"
version = "0.1.0"
type = "{plugin_type}"
language = "{language}"
entrypoint = "plugin.{ext}"
description = "Custom {plugin_type} plugin"
"#,
        ext = match language {
            "python" => "py",
            "go" => "go",
            "node" => "js",
            _ => "sh",
        }
    );
    std::fs::write(plugins_dir.join("devdock-plugin.toml"), metadata)?;

    // Generate plugin source
    match language {
        "python" => {
            std::fs::write(
                plugins_dir.join("plugin.py"),
                generate_python_plugin(name, plugin_type),
            )?;
            std::fs::write(plugins_dir.join("requirements.txt"), "# Add your dependencies here\n")?;
        }
        "go" => {
            std::fs::write(plugins_dir.join("plugin.go"), generate_go_plugin(name, plugin_type))?;
        }
        _ => {
            std::fs::write(plugins_dir.join("plugin.sh"), generate_bash_plugin(name, plugin_type))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    plugins_dir.join("plugin.sh"),
                    std::fs::Permissions::from_mode(0o755),
                )?;
            }
        }
    }

    output::success(&format!("Created plugin '{name}' at {}", plugins_dir.display()));
    println!();
    println!("  Files:");
    println!("    devdock-plugin.toml  — Plugin metadata");
    match language {
        "python" => {
            println!("    plugin.py            — Plugin implementation");
            println!("    requirements.txt     — Python dependencies");
        }
        "go" => println!("    plugin.go            — Plugin implementation"),
        _ => println!("    plugin.sh            — Plugin implementation"),
    }
    println!();
    println!("  Configure in devdock.toml:");
    println!("    [{plugin_type}]");
    println!("    backend = \"external\"");
    println!("    plugin = \"{}\"", plugins_dir.display());

    Ok(())
}

fn generate_python_plugin(name: &str, plugin_type: &str) -> String {
    format!(
        r#"#!/usr/bin/env python3
"""DevDock {plugin_type} plugin: {name}

Protocol: JSON-lines over stdin/stdout.
Receives: {{"method": "initialize"|"emit"|"shutdown", ...}}
Responds: {{"status": "ok"}} or {{"status": "error", "message": "..."}}
"""

import json
import sys


class DevDockPlugin:
    """Base class with protocol handling."""

    def run(self):
        for line in sys.stdin:
            try:
                msg = json.loads(line.strip())
                method = msg.get("method", "")

                if method == "initialize":
                    self.handle_initialize(msg.get("config", {{}}))
                    self._respond_ok()
                elif method == "emit":
                    self.handle_emit(msg.get("event", {{}}))
                    self._respond_ok()
                elif method == "shutdown":
                    self.handle_shutdown()
                    self._respond_ok()
                    break
                else:
                    self._respond_error(f"Unknown method: {{method}}")
            except Exception as e:
                self._respond_error(str(e))

    def _respond_ok(self):
        print(json.dumps({{"status": "ok"}}), flush=True)

    def _respond_error(self, message):
        print(json.dumps({{"status": "error", "message": message}}), flush=True)

    def handle_initialize(self, config):
        """Override: set up your plugin with the given config."""
        pass

    def handle_emit(self, event):
        """Override: handle a telemetry/diagnostic event."""
        pass

    def handle_shutdown(self):
        """Override: clean up resources."""
        pass


class {class_name}(DevDockPlugin):
    def handle_initialize(self, config):
        # YOUR CODE HERE
        pass

    def handle_emit(self, event):
        # YOUR CODE HERE
        event_type = event.get("type", "unknown")
        print(f"[{{event_type}}] {{event}}", file=sys.stderr)

    def handle_shutdown(self):
        pass


if __name__ == "__main__":
    {class_name}().run()
"#,
        class_name = name
            .chars()
            .enumerate()
            .map(|(i, c)| if i == 0 || name.as_bytes().get(i.wrapping_sub(1)) == Some(&b'_') {
                c.to_uppercase().to_string()
            } else if c == '_' {
                String::new()
            } else {
                c.to_string()
            })
            .collect::<String>()
            + "Plugin"
    )
}

fn generate_go_plugin(_name: &str, _plugin_type: &str) -> String {
    r#"package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

type Message struct {
	Method string                 `json:"method"`
	Config map[string]interface{} `json:"config,omitempty"`
	Event  map[string]interface{} `json:"event,omitempty"`
}

type Response struct {
	Status  string `json:"status"`
	Message string `json:"message,omitempty"`
}

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		var msg Message
		if err := json.Unmarshal(scanner.Bytes(), &msg); err != nil {
			respond("error", err.Error())
			continue
		}

		switch msg.Method {
		case "initialize":
			// YOUR CODE HERE
			respond("ok", "")
		case "emit":
			// YOUR CODE HERE
			fmt.Fprintf(os.Stderr, "[%s] %v\n", msg.Event["type"], msg.Event)
			respond("ok", "")
		case "shutdown":
			respond("ok", "")
			return
		default:
			respond("error", "unknown method: "+msg.Method)
		}
	}
}

func respond(status, message string) {
	resp := Response{Status: status, Message: message}
	data, _ := json.Marshal(resp)
	fmt.Println(string(data))
}
"#
    .to_string()
}

fn generate_bash_plugin(_name: &str, _plugin_type: &str) -> String {
    r#"#!/bin/bash
# DevDock external plugin (bash)
# Protocol: JSON-lines over stdin/stdout

while IFS= read -r line; do
    method=$(echo "$line" | python3 -c "import json,sys; print(json.load(sys.stdin).get('method',''))")

    case "$method" in
        initialize)
            echo '{"status":"ok"}'
            ;;
        emit)
            # YOUR CODE HERE
            echo '{"status":"ok"}'
            ;;
        shutdown)
            echo '{"status":"ok"}'
            exit 0
            ;;
        *)
            echo "{\"status\":\"error\",\"message\":\"unknown method: $method\"}"
            ;;
    esac
done
"#
    .to_string()
}
