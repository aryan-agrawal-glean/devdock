//! MCP server loop — reads JSON-RPC from stdin, dispatches to tools, writes responses to stdout.

use crate::tools;
use anyhow::Result;
use serde_json::Value;
use std::io::{self, BufRead, Write};

/// MCP server protocol version.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server, reading from stdin and writing to stdout.
pub async fn run() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            Ok(_) => continue,
            Err(e) => {
                tracing::error!("stdin read error: {e}");
                break;
            }
        };

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let error_response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {e}"),
                    }
                });
                let _ = writeln!(stdout, "{error_response}");
                let _ = stdout.flush();
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request["method"].as_str().unwrap_or("");

        let response = match method {
            "initialize" => handle_initialize(&id),
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tools_call(&id, &request).await,
            "notifications/initialized" => continue, // client notification, no response
            _ => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {method}"),
                    }
                })
            }
        };

        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }

    Ok(())
}

fn handle_initialize(id: &Option<Value>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "devdock",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }
    })
}

fn handle_tools_list(id: &Option<Value>) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": tools::list_tools()
        }
    })
}

async fn handle_tools_call(id: &Option<Value>, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or(serde_json::json!({}));
    let tool_name = params["name"].as_str().unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    match tools::call_tool(tool_name, &arguments).await {
        Ok(result) => {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": result.to_string()
                    }]
                }
            })
        }
        Err(e) => {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": format!("Error: {e}")
                    }],
                    "isError": true
                }
            })
        }
    }
}
