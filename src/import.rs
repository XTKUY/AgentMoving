//! Plan first, then publish new files without overwrites. Receipts support conservative rollback.
use crate::{
    agents,
    archive::{self, Bundle},
    model::*,
    paths,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub roots: BTreeMap<Agent, PathBuf>,
    pub maps: Vec<(String, String)>,
    pub selected: BTreeSet<String>,
    pub skip_conflicts: bool,
    pub allow_missing_projects: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    pub source: PathBuf,
    pub target: PathBuf,
    pub sha256: String,
    pub exists: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedSession {
    pub agent: Agent,
    pub id: String,
    pub root: PathBuf,
    pub project: String,
    pub status: String,
    pub files: Vec<PlannedFile>,
    pub notes: Vec<String>,
}
pub struct Plan {
    pub sessions: Vec<PlannedSession>,
    pub staging: tempfile::TempDir,
}

fn target_relative(s: &Session, payload: &Payload, project: &str) -> Result<String> {
    if project == s.project && s.agent != Agent::Pi {
        return Ok(payload.relative_path.clone());
    }
    match s.agent {
        Agent::Codex => Ok(payload.relative_path.clone()),
        Agent::ClaudeCode => {
            if payload.relative_path.starts_with("file-history/") {
                return Ok(payload.relative_path.clone());
            }
            let tail = payload
                .relative_path
                .splitn(3, '/')
                .nth(2)
                .context("Invalid Claude path")?;
            Ok(format!(
                "projects/{}/{tail}",
                paths::claude_project(project)?
            ))
        }
        Agent::Pi => Ok(format!(
            "{}/{}",
            paths::pi_project(project),
            payload
                .relative_path
                .rsplit('/')
                .next()
                .context("Invalid Pi path")?
        )),
    }
}

fn transform(
    source: &Path,
    target: &Path,
    agent: Agent,
    maps: &[(String, String)],
    parents: &BTreeMap<String, String>,
) -> Result<()> {
    let mut out = File::create(target)?;
    agents::each_record(source, |value, raw| {
        let Some(mut v) = value else {
            out.write_all(raw)?;
            return Ok(());
        };
        let original = v.clone();
        let kind = v["type"].as_str().unwrap_or("").to_owned();
        let metadata = match agent {
            Agent::Codex if kind == "session_meta" || kind == "turn_context" => {
                v.get_mut("payload")
            }
            Agent::ClaudeCode => Some(&mut v),
            Agent::Pi if kind == "session" => Some(&mut v),
            _ => None,
        };
        if let Some(m) = metadata {
            if let Some(cwd) = m["cwd"].as_str() {
                let mapped = paths::map_project(cwd, maps);
                m["cwd"] = mapped.into();
            }
            if agent == Agent::Pi
                && let Some(parent) = m["parentSession"].as_str()
                && let Some(mapped) = parents.get(&paths::normalize_foreign(parent))
            {
                m["parentSession"] = mapped.clone().into();
            }
        }
        if v == original {
            out.write_all(raw)?;
        } else {
            serde_json::to_writer(&mut out, &v)?;
            out.write_all(b"\n")?;
        }
        Ok(())
    })?;
    out.sync_all()?;
    Ok(())
}

pub fn plan(bundle: &Bundle, options: &ImportOptions, progress: &dyn Fn(String)) -> Result<Plan> {
    let mut map_sources = BTreeSet::new();
    for (from, to) in &options.maps {
        ensure!(!from.is_empty() && !to.is_empty(), "Empty path mapping");
        ensure!(
            map_sources.insert(paths::normalize_foreign(from)),
            "Duplicate source path mapping: {from}"
        );
    }
    let staging = tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)?;
    let mut targets = BTreeMap::new();
    let mut scans = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let selected: Vec<_> = bundle
        .manifest
        .sessions
        .iter()
        .filter(|s| options.selected.is_empty() || options.selected.contains(&s.session.key()))
        .collect();
    ensure!(!selected.is_empty(), "No matching sessions in bundle");
    for p in &selected {
        let a = p.session.agent;
        if let std::collections::btree_map::Entry::Vacant(e) = targets.entry(a) {
            let root = paths::absolute(
                &options
                    .roots
                    .get(&a)
                    .cloned()
                    .unwrap_or(agents::default_root(a)?),
            )?;
            paths::no_symlinks(&root)?;
            let scanned = agents::scan(vec![AgentRoot {
                agent: a,
                path: root.clone(),
                executable: agents::executable(a),
            }]);
            // Unknown/corrupt target files must not silently hide ID collisions.
            ensure!(
                scanned.warnings.is_empty(),
                "Target scan incomplete: {}",
                scanned.warnings.join("; ")
            );
            scans.insert(a, scanned);
            e.insert(root);
        }
        if p.session.agent == Agent::Pi {
            let project = paths::map_project(&p.session.project, &options.maps);
            let main = p
                .files
                .iter()
                .find(|f| f.relative_path == p.session.relative_path)
                .unwrap();
            let dest = targets[&a].join(paths::safe_relative(&target_relative(
                &p.session, main, &project,
            )?)?);
            parents.insert(
                paths::normalize_foreign(&p.session.source_path.to_string_lossy()),
                dest.to_string_lossy().into_owned(),
            );
        }
    }
    let mut sessions = Vec::new();
    let mut paths_seen = BTreeSet::new();
    for (idx, packed) in selected.iter().enumerate() {
        let s = &packed.session;
        progress(format!("Plan {}/{}: {}", idx + 1, selected.len(), s.key()));
        let project = paths::map_project(&s.project, &options.maps);
        let root = targets[&s.agent].clone();
        let mut planned = PlannedSession {
            agent: s.agent,
            id: s.id.clone(),
            root: root.clone(),
            project: project.clone(),
            status: "new".into(),
            files: Vec::new(),
            notes: Vec::new(),
        };
        if !Path::new(&project).is_absolute() || !Path::new(&project).is_dir() {
            planned
                .notes
                .push(format!("Project unavailable on target: {project}"));
            if !options.allow_missing_projects {
                planned.status = "blocked".into();
            }
        }
        if agents::executable(s.agent).is_none() {
            planned.notes.push(
                "Agent executable not found; files can be installed, native resume is unverified"
                    .into(),
            );
        }
        if s.agent == Agent::Codex {
            planned.notes.push("Native index/database is not overwritten. Restart Codex; verify with codex resume <id>. Desktop sidebar visibility is version-dependent.".into());
        }
        if s.agent == Agent::ClaudeCode {
            planned.notes.push("Claude retention settings can remove older transcripts; check cleanupPeriodDays before opening the target agent.".into());
        }
        if s.agent == Agent::Pi
            && let Some(parent) = &s.parent_session
            && !parents.contains_key(&paths::normalize_foreign(parent))
        {
            planned.notes.push(format!(
                "Parent session not selected or unavailable: {parent}"
            ));
            planned.status = "blocked".into();
        }
        for (j, payload) in packed.files.iter().enumerate() {
            let target = root.join(paths::safe_relative(&target_relative(
                s, payload, &project,
            )?)?);
            paths::no_symlinks(&target)?;
            ensure!(
                paths_seen.insert(target.to_string_lossy().to_lowercase()),
                "Multiple payloads map to the same destination: {}",
                target.display()
            );
            let source = staging.path().join(format!("{idx}-{j}"));
            if payload.transcript
                && (!options.maps.is_empty() || s.agent == Agent::Pi && s.parent_session.is_some())
            {
                transform(
                    &bundle.file(payload),
                    &source,
                    s.agent,
                    &options.maps,
                    &parents,
                )?;
            } else {
                fs::copy(bundle.file(payload), &source)?;
            }
            let digest = archive::hash_file(&source)?;
            let exists = target.exists();
            if exists && (!target.is_file() || archive::hash_file(&target)? != digest) {
                planned.status = "conflict".into();
                planned
                    .notes
                    .push(format!("Different existing file: {}", target.display()));
            }
            planned.files.push(PlannedFile {
                source,
                target,
                sha256: digest,
                exists,
            });
        }
        let main_target = &planned.files[packed
            .files
            .iter()
            .position(|f| f.relative_path == s.relative_path)
            .unwrap()]
        .target;
        for existing in scans[&s.agent].sessions.iter().filter(|e| e.id == s.id) {
            if existing.source_path != *main_target {
                planned.status = "conflict".into();
                planned.notes.push(format!(
                    "Session ID already exists at {}",
                    existing.source_path.display()
                ));
            }
        }
        if planned.status == "new" && planned.files.iter().all(|f| f.exists) {
            planned.status = "identical".into();
        }
        sessions.push(planned);
    }
    Ok(Plan { sessions, staging })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptFile {
    pub target: PathBuf,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub version: u32,
    pub status: String,
    pub created_at: String,
    pub roots: Vec<PathBuf>,
    pub files: Vec<ReceiptFile>,
    pub notes: Vec<String>,
}
struct Locks(Vec<PathBuf>);
impl Drop for Locks {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_file(p);
        }
    }
}
fn acquire(roots: &[PathBuf]) -> Result<Locks> {
    let mut locks = Locks(Vec::new());
    for root in roots {
        paths::no_symlinks(root)?;
        fs::create_dir_all(root)?;
        let lock = root.join(".agentmoving.lock");
        let mut file = OpenOptions::new().write(true).create_new(true).open(&lock).with_context(|| format!("Migration lock exists or cannot be created: {}. If a previous process crashed, ensure it has stopped before removing this lock and rolling back its receipt.", lock.display()))?;
        locks.0.push(lock);
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
    }
    Ok(locks)
}
fn save_receipt(path: &Path, receipt: &Receipt) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(receipt)?;
    ensure!(
        bytes.len() as u64 <= MAX_MANIFEST,
        "Transaction receipt exceeds 16 MiB; import fewer sessions at once"
    );
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn apply(plan: &Plan, options: &ImportOptions, progress: &dyn Fn(String)) -> Result<PathBuf> {
    ensure!(
        !plan.sessions.iter().any(|s| s.status == "blocked"),
        "Import blocked: map missing projects (or --allow-missing-projects) and include Pi parent sessions"
    );
    ensure!(
        options.skip_conflicts || !plan.sessions.iter().any(|s| s.status == "conflict"),
        "Conflicts found; no files written. Use --skip-conflicts to skip whole conflicting sessions"
    );
    // Never import a child whose bundled parent was skipped due to a conflict.
    if options.skip_conflicts
        && plan
            .sessions
            .iter()
            .any(|s| s.agent == Agent::Pi && s.status == "conflict")
    {
        bail!(
            "Pi conflicts require resolving the conflict before import to preserve parent references"
        );
    }
    let sessions: Vec<_> = plan.sessions.iter().filter(|s| s.status == "new").collect();
    ensure!(
        !sessions.is_empty(),
        "Nothing to import; all selected sessions are identical or skipped"
    );
    let roots: Vec<_> = sessions
        .iter()
        .map(|s| s.root.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let _locks = acquire(&roots)?;
    // Re-check the complete plan while locked, including files initially classified as identical.
    for s in &sessions {
        for f in &s.files {
            paths::no_symlinks(&f.target)?;
            if f.exists {
                ensure!(
                    f.target.is_file() && archive::hash_file(&f.target)? == f.sha256,
                    "Target changed after planning"
                );
            } else {
                ensure!(
                    !f.target.exists(),
                    "Target appeared after planning: {}",
                    f.target.display()
                );
            }
        }
    }
    let journals = roots[0].join(".agentmoving/transactions");
    paths::no_symlinks(&journals)?;
    fs::create_dir_all(&journals)?;
    let file = tempfile::Builder::new()
        .prefix("import-")
        .suffix(".json")
        .tempfile_in(&journals)?;
    let (_, receipt_path) = file.keep()?;
    let mut receipt = Receipt {
        version: 1,
        status: "in-progress".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        roots,
        files: Vec::new(),
        notes: Vec::new(),
    };
    save_receipt(&receipt_path, &receipt)?;
    let result = (|| -> Result<()> {
        for s in sessions {
            for f in &s.files {
                if f.exists {
                    continue;
                }
                progress(format!("Import {}: {}", s.id, f.target.display()));
                paths::no_symlinks(&f.target)?;
                fs::create_dir_all(f.target.parent().unwrap())?;
                let mut staged = tempfile::NamedTempFile::new_in(f.target.parent().unwrap())?;
                std::io::copy(&mut File::open(&f.source)?, staged.as_file_mut())?;
                staged.as_file().sync_all()?;
                // Write-ahead receipt makes crash recovery possible between publication and the next file.
                receipt.files.push(ReceiptFile {
                    target: f.target.clone(),
                    sha256: f.sha256.clone(),
                });
                save_receipt(&receipt_path, &receipt)?;
                if let Err(e) = staged.persist_noclobber(&f.target) {
                    receipt.files.pop();
                    save_receipt(&receipt_path, &receipt)?;
                    return Err(e.error.into());
                }
                ensure!(
                    archive::hash_file(&f.target)? == f.sha256,
                    "Post-write checksum failed"
                );
            }
        }
        receipt.status = "completed".into();
        save_receipt(&receipt_path, &receipt)?;
        Ok(())
    })();
    if let Err(error) = result {
        receipt.status = "failed".into();
        receipt.notes.push(error.to_string());
        let rollback = undo_files(&mut receipt);
        let journal = save_receipt(&receipt_path, &receipt);
        bail!(
            "Import failed: {error:#}. Rollback: {rollback:?}. Journal update: {journal:?}. Receipt: {}",
            receipt_path.display()
        );
    }
    Ok(receipt_path)
}
fn undo_files(receipt: &mut Receipt) -> Result<()> {
    let mut retained = Vec::new();
    for f in receipt.files.iter().rev() {
        let outcome = (|| -> Result<()> {
            paths::no_symlinks(&f.target)?;
            if !f.target.exists() {
                return Ok(());
            }
            ensure!(
                archive::hash_file(&f.target)? == f.sha256,
                "Content changed"
            );
            fs::remove_file(&f.target)?;
            Ok(())
        })();
        if let Err(error) = outcome {
            retained.push(format!("{}: {error}", f.target.display()));
        }
    }
    receipt.status = if retained.is_empty() {
        "rolled-back"
    } else {
        "rollback-incomplete"
    }
    .into();
    receipt.notes.extend(
        retained
            .iter()
            .map(|p| format!("Retained changed file: {p}")),
    );
    ensure!(
        retained.is_empty(),
        "Some imported files were modified; retained them"
    );
    Ok(())
}
pub fn rollback(path: &Path) -> Result<Receipt> {
    paths::no_symlinks(path)?;
    let mut receipt: Receipt =
        serde_json::from_reader(std::io::Read::take(File::open(path)?, MAX_MANIFEST))?;
    ensure!(
        receipt.version == 1 && !receipt.roots.is_empty(),
        "Unsupported receipt"
    );
    ensure!(
        paths::absolute(path)?.parent()
            == Some(receipt.roots[0].join(".agentmoving/transactions").as_path()),
        "Receipt is not in its original transaction directory"
    );
    for f in &receipt.files {
        let root = receipt
            .roots
            .iter()
            .find(|r| f.target.starts_with(r))
            .context("Receipt target outside roots")?;
        let relative = paths::portable(f.target.strip_prefix(root)?)?;
        ensure!(
            !relative.starts_with('.')
                && (relative.starts_with("sessions/")
                    || relative.starts_with("archived_sessions/")
                    || relative.starts_with("projects/")
                    || relative.starts_with("file-history/")
                    || relative.starts_with("--")),
            "Receipt target is not a session file"
        );
    }
    let _locks = acquire(&receipt.roots)?;
    let result = undo_files(&mut receipt);
    save_receipt(path, &receipt)?;
    result?;
    Ok(receipt)
}
