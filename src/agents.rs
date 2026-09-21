//! Discovery and version-tolerant readers for native transcripts.
use crate::{model::*, paths};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub fn executable(agent: Agent) -> Option<PathBuf> {
    let mut dirs: Vec<_> = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    if let Ok(home) = paths::home() {
        dirs.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    for dir in dirs {
        for suffix in if cfg!(windows) {
            vec![".exe", ".cmd", ".bat", ""]
        } else {
            vec![""]
        } {
            let p = dir.join(format!("{}{suffix}", agent.command()));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
pub fn default_root(agent: Agent) -> Result<PathBuf> {
    let home = paths::home()?;
    Ok(match agent {
        Agent::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".codex")),
        Agent::ClaudeCode => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or(home.join(".claude")),
        Agent::Pi => {
            if let Some(p) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
                return Ok(p.into());
            }
            let config = std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .unwrap_or(home.join(".pi/agent"));
            if let Ok(f) = File::open(config.join("settings.json"))
                && let Ok(v) = serde_json::from_reader::<_, Value>(f.take(MAX_MANIFEST))
                && let Some(p) = v.get("sessionDir").and_then(Value::as_str)
            {
                let p = if let Some(tail) = p.strip_prefix("~/") {
                    home.join(tail)
                } else {
                    PathBuf::from(p)
                };
                if p.is_absolute() {
                    return Ok(p);
                }
            }
            config.join("sessions")
        }
    })
}
pub fn roots(agent: Option<Agent>, explicit: &[PathBuf]) -> Result<Vec<AgentRoot>> {
    ensure!(
        explicit.is_empty() || agent.is_some(),
        "--root requires --agent"
    );
    let mut out = Vec::new();
    for a in Agent::ALL
        .into_iter()
        .filter(|a| agent.is_none_or(|x| x == *a))
    {
        let mut candidates = if explicit.is_empty() {
            vec![default_root(a)?]
        } else {
            explicit.to_vec()
        };
        // Include the conventional location even when an environment override is active.
        if explicit.is_empty() {
            let h = paths::home()?;
            candidates.push(h.join(match a {
                Agent::Codex => ".codex",
                Agent::ClaudeCode => ".claude",
                Agent::Pi => ".pi/agent/sessions",
            }));
        }
        for p in candidates {
            let p = paths::absolute(&p)?;
            if !out.iter().any(|r: &AgentRoot| r.agent == a && r.path == p) {
                out.push(AgentRoot {
                    agent: a,
                    path: p,
                    executable: executable(a),
                });
            }
        }
    }
    Ok(out)
}
/// Read bounded JSONL records without loading an entire session into memory.
pub fn each_json(path: &Path, mut visit: impl FnMut(Value) -> Result<()>) -> Result<()> {
    each_record(path, |value, _| {
        if let Some(value) = value {
            visit(value)?;
        }
        Ok(())
    })
}

pub fn each_record(
    path: &Path,
    mut visit: impl FnMut(Option<Value>, &[u8]) -> Result<()>,
) -> Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut count = 0;
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        count += 1;
        ensure!(
            n as u64 <= MAX_LINE,
            "JSONL line exceeds 64 MiB: {}:{count}",
            path.display()
        );
        if line.iter().all(u8::is_ascii_whitespace) {
            visit(None, &line)?;
            continue;
        }
        let value: Value = serde_json::from_slice(&line).with_context(|| {
            format!(
                "Invalid JSONL {}:{count}; stop the active session and retry",
                path.display()
            )
        })?;
        ensure!(value.is_object(), "JSONL record must be an object");
        visit(Some(value), &line)?;
    }
    Ok(())
}
fn text_content(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.chars().take(160).collect();
    }
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .flat_map(str::chars)
                .take(160)
                .collect()
        })
        .unwrap_or_default()
}
pub fn read_session(agent: Agent, root: &Path, path: &Path) -> Result<Session> {
    paths::no_symlinks(path)?;
    let before = path.metadata()?;
    ensure!(before.len() <= MAX_FILE, "Session exceeds 2 GiB");
    let relative = paths::portable(path.strip_prefix(root)?)?;
    let mut id = String::new();
    let mut project = String::new();
    let mut title = String::new();
    let mut timestamp = String::new();
    let mut agent_version = None;
    let mut format_version = None;
    let mut parent = None;
    let mut headers = 0;
    each_json(path, |v| {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match agent {
            Agent::Codex => {
                if kind == "session_meta" {
                    headers += 1;
                    ensure!(headers == 1, "Multiple Codex session headers");
                    let p = &v["payload"];
                    id = p["id"].as_str().unwrap_or("").to_owned();
                    project = p["cwd"].as_str().unwrap_or("").to_owned();
                    timestamp = p["timestamp"]
                        .as_str()
                        .or(v["timestamp"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    agent_version = p["cli_version"].as_str().map(str::to_owned);
                }
                if title.is_empty() && kind == "event_msg" && v["payload"]["type"] == "user_message"
                {
                    title = text_content(&v["payload"]["message"]);
                }
                if title.is_empty() && kind == "response_item" && v["payload"]["role"] == "user" {
                    title = text_content(&v["payload"]["content"]);
                }
            }
            Agent::ClaudeCode => {
                if let Some(record_id) = v["sessionId"].as_str() {
                    ensure!(
                        id.is_empty() || id == record_id,
                        "Mixed Claude session IDs in one file"
                    );
                }
                if id.is_empty() {
                    id = v["sessionId"].as_str().unwrap_or("").to_owned();
                }
                if project.is_empty() {
                    project = v["cwd"].as_str().unwrap_or("").to_owned();
                }
                if timestamp.is_empty() {
                    timestamp = v["timestamp"].as_str().unwrap_or("").to_owned();
                }
                if agent_version.is_none() {
                    agent_version = v["version"].as_str().map(str::to_owned);
                }
                if title.is_empty() && kind == "user" {
                    title = text_content(&v["message"]["content"]);
                }
                if kind == "custom-title" {
                    title = text_content(&v["customTitle"]);
                }
            }
            Agent::Pi => {
                if kind == "session" {
                    headers += 1;
                    ensure!(headers == 1, "Multiple Pi session headers");
                    id = v["id"].as_str().unwrap_or("").to_owned();
                    project = v["cwd"].as_str().unwrap_or("").to_owned();
                    timestamp = v["timestamp"].as_str().unwrap_or("").to_owned();
                    format_version = v["version"].as_u64();
                    ensure!(
                        matches!(format_version, Some(1..=3)),
                        "Unsupported Pi session version: {format_version:?}"
                    );
                    parent = v["parentSession"].as_str().map(str::to_owned);
                }
                if title.is_empty() && kind == "message" && v["message"]["role"] == "user" {
                    title = text_content(&v["message"]["content"]);
                }
                if kind == "session_info" && v["name"].is_string() {
                    title = text_content(&v["name"]);
                }
            }
        }
        Ok(())
    })?;
    ensure!(
        !id.is_empty() && !project.is_empty(),
        "Missing session identity or cwd"
    );
    paths::safe_relative(&id)?;
    ensure!(!id.contains('/'), "Session ID must be a single component");
    let after = path.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Session changed during scan: {}",
        path.display()
    );
    if title.is_empty() {
        title = id.clone();
    }
    Ok(Session {
        agent,
        id,
        project,
        title: paths::clean_display(&title),
        timestamp,
        agent_version,
        format_version,
        archived: relative.starts_with("archived_sessions/"),
        relative_path: relative,
        source_path: path.to_owned(),
        source_root: root.to_owned(),
        parent_session: parent,
        bytes: before.len(),
    })
}
pub fn scan(roots: Vec<AgentRoot>) -> Scan {
    let mut out = Scan {
        roots: roots.clone(),
        ..Scan::default()
    };
    for root in roots {
        if !root.path.exists() {
            continue;
        }
        if let Err(e) = paths::no_symlinks(&root.path) {
            out.warnings.push(e.to_string());
            continue;
        }
        let dirs = match root.agent {
            Agent::Codex => vec![
                root.path.join("sessions"),
                root.path.join("archived_sessions"),
            ],
            Agent::ClaudeCode => vec![root.path.join("projects")],
            Agent::Pi => vec![root.path.clone()],
        };
        for dir in dirs.into_iter().filter(|p| p.exists()) {
            for entry in WalkDir::new(dir).follow_links(false).max_depth(24) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        out.warnings.push(e.to_string());
                        continue;
                    }
                };
                if entry.file_type().is_symlink() {
                    out.warnings
                        .push(format!("Skipped symlink {}", entry.path().display()));
                    continue;
                }
                if !entry.file_type().is_file()
                    || entry.path().extension().is_none_or(|s| s != "jsonl")
                {
                    continue;
                }
                // Claude subagent transcripts are dependencies of the owning main session.
                if root.agent == Agent::ClaudeCode
                    && entry
                        .path()
                        .strip_prefix(&root.path)
                        .map_or(true, |p| p.components().count() != 3)
                {
                    continue;
                }
                match read_session(root.agent, &root.path, entry.path()) {
                    Ok(s) => out.sessions.push(s),
                    Err(e) => out
                        .warnings
                        .push(format!("{}: {e:#}", entry.path().display())),
                }
            }
        }
    }
    out.sessions
        .sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.id.cmp(&b.id)));
    out
}

/// Only agent-owned, session-scoped dependencies are included, never arbitrary paths in chat text.
pub fn related_files(session: &Session) -> Result<Vec<PathBuf>> {
    let mut files = vec![session.source_path.clone()];
    let mut dirs = Vec::new();
    if session.agent == Agent::ClaudeCode {
        dirs.push(session.source_path.with_extension(""));
        dirs.push(session.source_root.join("file-history").join(&session.id));
    }
    if session.agent == Agent::Pi {
        let sidecar = PathBuf::from(format!("{}.acp.json", session.source_path.display()));
        if sidecar.exists() {
            files.push(sidecar);
        }
    }
    for dir in dirs.into_iter().filter(|p| p.exists()) {
        for entry in WalkDir::new(dir).follow_links(false).max_depth(24) {
            let e = entry?;
            ensure!(
                !e.file_type().is_symlink(),
                "Session dependency is a symlink: {}",
                e.path().display()
            );
            if e.file_type().is_file() {
                files.push(e.path().to_owned());
            }
        }
    }
    Ok(files)
}
