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

/// Map plugin type to the hooks most relevant for that type.
fn suggested_hooks(plugin_type: &str) -> Vec<&'static str> {
    match plugin_type {
        "telemetry" => vec!["after_tilt", "before_stop", "on_error"],
        "diagnostics" => vec!["on_error", "on_report"],
        "storage" => vec!["before_config_generated", "after_config_generated"],
        _ => vec!["after_ready", "on_error"], // notifications and unknown
    }
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
    let ext = match language {
        "python" => "py",
        "go" => "go",
        "node" => "js",
        "rust" => "rs",
        _ => "sh",
    };
    let metadata = format!(
        r#"[plugin]
name = "{name}"
version = "0.1.0"
type = "{plugin_type}"
language = "{language}"
entrypoint = "plugin.{ext}"
description = "Custom {plugin_type} plugin"
"#,
    );
    std::fs::write(plugins_dir.join("devdock-plugin.toml"), metadata)?;

    // Generate plugin source
    match language {
        "python" => {
            std::fs::write(
                plugins_dir.join("plugin.py"),
                generate_python_plugin(name, plugin_type),
            )?;
            std::fs::write(
                plugins_dir.join("requirements.txt"),
                "# Add your Python dependencies here\n",
            )?;
        }
        "go" => {
            std::fs::write(plugins_dir.join("plugin.go"), generate_go_plugin(name, plugin_type))?;
        }
        "rust" => {
            let src_dir = plugins_dir.join("src");
            std::fs::create_dir_all(&src_dir)?;
            std::fs::write(plugins_dir.join("Cargo.toml"), generate_rust_cargo_toml(name))?;
            std::fs::write(src_dir.join("main.rs"), generate_rust_plugin(name, plugin_type))?;
        }
        _ => {
            // bash / default
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

    let hooks = suggested_hooks(plugin_type);

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
        "rust" => {
            println!("    Cargo.toml           — Rust package manifest");
            println!("    src/main.rs          — Plugin implementation");
        }
        _ => println!("    plugin.sh            — Plugin implementation"),
    }

    println!();
    println!("  Configure in devdock.toml:");
    println!("    [plugins.hooks]");

    let entrypoint = match language {
        "python" => format!("python3 {}/plugin.py", plugins_dir.display()),
        "go" => format!("go run {}/plugin.go", plugins_dir.display()),
        "rust" => format!("{}/target/release/{name}", plugins_dir.display()),
        _ => format!("{}/plugin.sh", plugins_dir.display()),
    };

    for hook in &hooks {
        println!("    {hook} = [\"{entrypoint}\"]");
    }

    if language == "rust" {
        println!();
        println!("  Build before use:");
        println!("    cd {}", plugins_dir.display());
        println!("    cargo build --release");
    }

    println!();
    println!("  Available lifecycle hooks:");
    println!("    after_profile_loaded    before_infra_ready    after_infra_ready");
    println!("    before_config_generated after_config_generated");
    println!("    before_tilt             after_tilt");
    println!("    before_resource_start   after_resource_start");
    println!("    after_ready             before_stop           after_stop");
    println!("    on_error                on_report");

    Ok(())
}

/// Convert a plugin name (kebab-case) to a StructName (PascalCase).
/// Avoids doubling "Plugin" suffix if the name already ends with "plugin".
fn to_struct_name(name: &str) -> String {
    let normalized = name.replace('-', "_");
    let pascal: String = normalized
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + &chars.collect::<String>(),
            }
        })
        .collect();

    if pascal.ends_with("Plugin") {
        pascal
    } else {
        pascal + "Plugin"
    }
}

fn generate_python_plugin(name: &str, plugin_type: &str) -> String {
    let class_name = to_struct_name(name);
    let hooks = suggested_hooks(plugin_type);
    let hook_methods: String = hooks
        .iter()
        .map(|hook| {
            let doc = hook_doc_python(hook);
            format!(
                "\n    def {hook}(self, params):\n        {doc}\n        # YOUR CODE HERE\n        pass\n"
            )
        })
        .collect();

    let hook_dispatch: String = hooks
        .iter()
        .enumerate()
        .map(|(i, hook)| {
            let prefix = if i == 0 { "if" } else { "elif" };
            format!("        {prefix} method == \"{hook}\":\n            self.{hook}(params)\n")
        })
        .collect();

    format!(
        r#"#!/usr/bin/env python3
"""DevDock {plugin_type} plugin: {name}

Protocol: JSON-lines over stdin/stdout.
  In:  {{"method": "after_ready", "params": {{...}}}}
  Out: {{"status": "ok"}}

Available lifecycle hooks:
  after_profile_loaded, before_infra_ready, after_infra_ready,
  before_config_generated, after_config_generated,
  before_tilt, after_tilt, before_resource_start, after_resource_start,
  after_ready, before_stop, after_stop, on_error, on_report
"""

import json
import sys


def respond_ok():
    print(json.dumps({{"status": "ok"}}), flush=True)


def respond_error(message):
    print(json.dumps({{"status": "error", "message": message}}), flush=True)


class {class_name}:
    def initialize(self, config):
        """Called once with config from devdock.toml [plugins.config.{name}]."""
        pass
{hook_methods}
    def handle(self, method, params):
{hook_dispatch}        else:
            pass  # Return ok for unhandled hooks (see respond_ok below)


def run(plugin):
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
            method = msg.get("method", "")
            if method == "initialize":
                plugin.initialize(msg.get("config", {{}}))
                respond_ok()
            elif method == "shutdown":
                respond_ok()
                break
            else:
                plugin.handle(method, msg.get("params", {{}}))
                respond_ok()
        except Exception as e:
            respond_error(str(e))


if __name__ == "__main__":
    run({class_name}())
"#
    )
}

fn hook_doc_python(hook: &str) -> &'static str {
    match hook {
        "after_ready" => {
            "\"\"\"All resources healthy. params: profile_name, resource_count, startup_duration_secs.\"\"\""
        }
        "on_error" => "\"\"\"Unrecoverable error. params: session_id, error, state.\"\"\"",
        "before_stop" | "after_stop" => {
            "\"\"\"Session stopping/stopped. params: session_id, duration_secs.\"\"\""
        }
        "after_tilt" => "\"\"\"Tilt is running. params: session_id, profile_name.\"\"\"",
        "on_report" => "\"\"\"devdock report command. params: title, body.\"\"\"",
        "before_config_generated" | "after_config_generated" => {
            "\"\"\"Config phase. params: profile_name, runtime_dir, workspace_root.\"\"\""
        }
        "before_resource_start" | "after_resource_start" => {
            "\"\"\"Resource lifecycle. params: resource_name, resource_kind.\"\"\""
        }
        _ => "\"\"\"Lifecycle hook.\"\"\"",
    }
}

fn hook_doc_rust(hook: &str) -> &'static str {
    match hook {
        "after_ready" => {
            "// All resources healthy.\n                // params: profile_name, resource_count, startup_duration_secs"
        }
        "on_error" => "// Unrecoverable error.\n                // params: session_id, error, state",
        "before_stop" | "after_stop" => {
            "// Session stopping/stopped.\n                // params: session_id, duration_secs"
        }
        "after_tilt" => "// Tilt is running.\n                // params: session_id, profile_name",
        "on_report" => "// devdock report command.\n                // params: title, body",
        "before_config_generated" | "after_config_generated" => {
            "// Config phase.\n                // params: profile_name, runtime_dir, workspace_root"
        }
        "before_resource_start" | "after_resource_start" => {
            "// Resource lifecycle.\n                // params: resource_name, resource_kind"
        }
        _ => "// Lifecycle hook.",
    }
}

fn generate_rust_cargo_toml(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
devdock-plugin-sdk = {{ git = "https://github.com/aryan-agrawal-glean/devdock" }}
serde_json = "1.0"
"#
    )
}

fn generate_rust_plugin(name: &str, plugin_type: &str) -> String {
    let struct_name = to_struct_name(name);
    let hooks = suggested_hooks(plugin_type);

    let match_arms: String = hooks
        .iter()
        .map(|hook| {
            let doc = hook_doc_rust(hook);
            format!(
                "            \"{hook}\" => {{\n                {doc}\n                // YOUR CODE HERE\n                ok()\n            }}\n"
            )
        })
        .collect();

    format!(
        r#"use devdock_plugin_sdk::{{ok, Plugin}};
use serde_json::Value;

#[derive(Default)]
struct {struct_name};

impl Plugin for {struct_name} {{
    fn name(&self) -> &str {{
        "{name}"
    }}

    fn initialize(&mut self, config: Value) {{
        // Read config from devdock.toml [plugins.config.{name}]
        eprintln!("[{name}] initialized");
        let _ = config;
    }}

    fn handle(&self, method: &str, msg: &Value) -> Value {{
        let params = &msg["params"];
        match method {{
{match_arms}            _ => ok(), // Ignore unhandled hooks
        }}
    }}
}}

fn main() {{
    devdock_plugin_sdk::run::<{struct_name}>();
}}
"#
    )
}

fn generate_go_plugin(name: &str, plugin_type: &str) -> String {
    let hooks = suggested_hooks(plugin_type);

    let hook_functions: String = hooks
        .iter()
        .map(|hook| {
            let func_name = snake_to_camel(hook);
            format!(
                r#"
func {func_name}(params json.RawMessage) {{
    var p map[string]interface{{}}
    json.Unmarshal(params, &p)
    // YOUR CODE HERE
    fmt.Fprintf(os.Stderr, "[{name}] {hook}: %v\n", p)
}}
"#
            )
        })
        .collect();

    let switch_cases: String = hooks
        .iter()
        .map(|hook| {
            let func_name = snake_to_camel(hook);
            format!(
                "        case \"{hook}\":\n            {func_name}(msg.Params)\n            respondOK()\n"
            )
        })
        .collect();

    format!(
        r#"package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

// Message is the JSON-lines protocol message from devdock.
type Message struct {{
	Method string          `json:"method"`
	Config json.RawMessage `json:"config,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
}}

// Response is the JSON-lines protocol response to devdock.
type Response struct {{
	Status  string `json:"status"`
	Message string `json:"message,omitempty"`
}}

var pluginConfig map[string]interface{{}}

func respondOK() {{
	r, _ := json.Marshal(Response{{Status: "ok"}})
	fmt.Println(string(r))
}}

func respondError(message string) {{
	r, _ := json.Marshal(Response{{Status: "error", Message: message}})
	fmt.Println(string(r))
}}

func initialize(cfg json.RawMessage) {{
	// Read config from devdock.toml [plugins.config.{name}]
	json.Unmarshal(cfg, &pluginConfig)
	fmt.Fprintf(os.Stderr, "[{name}] initialized\n")
}}
{hook_functions}
func main() {{
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {{
		var msg Message
		if err := json.Unmarshal(scanner.Bytes(), &msg); err != nil {{
			respondError(err.Error())
			continue
		}}
		switch msg.Method {{
		case "initialize":
			initialize(msg.Config)
			respondOK()
{switch_cases}		case "shutdown":
			respondOK()
			return
		default:
			respondOK() // Ignore unhandled hooks
		}}
	}}
}}
"#
    )
}

/// Convert snake_case hook name to camelCase Go function name.
fn snake_to_camel(s: &str) -> String {
    s.split('_')
        .enumerate()
        .map(|(i, word)| {
            if i == 0 {
                word.to_string()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(c) => c.to_uppercase().to_string() + &chars.collect::<String>(),
                }
            }
        })
        .collect()
}

fn generate_bash_plugin(name: &str, plugin_type: &str) -> String {
    let hooks = suggested_hooks(plugin_type);

    let hook_functions: String = hooks
        .iter()
        .map(|hook| {
            format!(
                r#"
{hook}() {{
    local params="$1"
    # YOUR CODE HERE
    echo "[{name}] {hook}" >&2
}}
"#
            )
        })
        .collect();

    let case_entries: String = hooks
        .iter()
        .map(|hook| {
            format!(
                "        {hook})\n            params=$(echo \"$line\" | jq -c '.params // {{}}')\n            {hook} \"$params\"\n            echo '{{\"status\":\"ok\"}}'\n            ;;\n"
            )
        })
        .collect();

    format!(
        r#"#!/usr/bin/env bash
# DevDock plugin: {name} ({plugin_type})
# Protocol: JSON-lines over stdin/stdout.
#   In:  {{"method": "after_ready", "params": {{...}}}}
#   Out: {{"status": "ok"}}
# Requires: jq

set -euo pipefail

if ! command -v jq &>/dev/null; then
    echo '{{"status":"error","message":"jq is required (brew install jq / apt-get install jq)"}}' >&1
    exit 1
fi

CONFIG='{{}}'
{hook_functions}
while IFS= read -r line; do
    [[ -z "$line" ]] && continue

    method=$(echo "$line" | jq -r '.method // empty')

    case "$method" in
        initialize)
            CONFIG=$(echo "$line" | jq -c '.config // {{}}')
            echo '{{"status":"ok"}}'
            ;;
{case_entries}        shutdown)
            echo '{{"status":"ok"}}'
            exit 0
            ;;
        *)
            echo '{{"status":"ok"}}' # Ignore unhandled hooks
            ;;
    esac
done
"#
    )
}
