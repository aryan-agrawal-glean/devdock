//! DevDock MCP Server
//!
//! An MCP (Model Context Protocol) server that exposes DevDock as tools for AI agents
//! (Claude Code, Cursor, Codex, etc.). This makes DevDock controllable by AI — start
//! environments, check health, generate plugins, read logs — all from an agent context.

pub mod server;
pub mod tools;
