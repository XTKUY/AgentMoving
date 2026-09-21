//! Portable metadata; native JSONL remains the authoritative payload.
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    Codex,
    ClaudeCode,
    Pi,
}
impl Agent {
    pub const ALL: [Self; 3] = [Self::Codex, Self::ClaudeCode, Self::Pi];
    pub fn command(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Pi => "pi",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::Pi => "pi",
        }
    }
}
impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoot {
    pub agent: Agent,
    /// Codex/Claude: configuration root. Pi: session directory itself.
    pub path: PathBuf,
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub agent: Agent,
    pub id: String,
    pub project: String,
    pub title: String,
    pub timestamp: String,
    pub agent_version: Option<String>,
    pub format_version: Option<u64>,
    pub archived: bool,
    pub relative_path: String,
    pub source_path: PathBuf,
    pub source_root: PathBuf,
    pub parent_session: Option<String>,
    pub bytes: u64,
}
impl Session {
    pub fn key(&self) -> String {
        format!("{}:{}", self.agent, self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Scan {
    pub roots: Vec<AgentRoot>,
    pub sessions: Vec<Session>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub entry: String,
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub transcript: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedSession {
    pub session: Session,
    pub files: Vec<Payload>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub created_at: String,
    pub source_os: String,
    pub tool_version: String,
    pub sessions: Vec<PackedSession>,
    pub warnings: Vec<String>,
}

pub const MAX_LINE: u64 = 64 * 1024 * 1024;
pub const MAX_FILE: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_TOTAL: u64 = 20 * 1024 * 1024 * 1024;
pub const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
pub const MAX_FILES: usize = 100_000;
