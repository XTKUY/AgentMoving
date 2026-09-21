//! Versioned ZIP bundles, bounded extraction and byte-for-byte integrity verification.
use crate::{agents, model::*, paths};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::{NamedTempFile, TempDir};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub fn hash_file(path: &Path) -> Result<String> {
    let mut reader = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn export(
    selected: &[Session],
    all: &[Session],
    output: &Path,
    progress: &dyn Fn(String),
) -> Result<Manifest> {
    export_with_warnings(selected, all, output, &[], progress)
}

pub fn export_with_warnings(
    selected: &[Session],
    all: &[Session],
    output: &Path,
    scan_warnings: &[String],
    progress: &dyn Fn(String),
) -> Result<Manifest> {
    ensure!(!selected.is_empty(), "No sessions selected");
    let output = paths::absolute(output)?;
    paths::no_symlinks(&output)?;
    ensure!(
        !output.exists(),
        "Output already exists: {}",
        output.display()
    );
    let parent = output.parent().context("Output has no parent")?;
    ensure!(parent.is_dir(), "Output directory does not exist");
    let mut sessions = selected.to_vec();
    let mut warnings = vec!["Credentials, project source code and external file references are excluded. Native restore depends on the target agent version.".to_owned()];
    warnings.extend_from_slice(scan_warnings);
    let mut i = 0;
    while i < sessions.len() {
        if sessions[i].agent == Agent::Pi
            && let Some(parent) = &sessions[i].parent_session
        {
            let parent = paths::normalize_foreign(parent);
            if let Some(found) = all.iter().find(|s| {
                s.agent == Agent::Pi
                    && paths::normalize_foreign(&s.source_path.to_string_lossy()) == parent
            }) {
                if !sessions.iter().any(|s| s.source_path == found.source_path) {
                    sessions.push(found.clone());
                }
            } else {
                warnings.push(format!("Pi parent session missing from scan: {parent}"));
            }
        }
        i += 1;
    }
    let mut seen = BTreeSet::new();
    for s in &sessions {
        ensure!(
            seen.insert(s.key()),
            "Duplicate session ID across source roots: {}. Export the roots separately.",
            s.key()
        );
    }
    let mut manifest = Manifest {
        format: "agentmoving".into(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        source_os: std::env::consts::OS.into(),
        tool_version: env!("CARGO_PKG_VERSION").into(),
        sessions: Vec::new(),
        warnings,
    };
    let mut temp = NamedTempFile::new_in(parent)?;
    let mut writer = ZipWriter::new(temp.as_file_mut());
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600)
        .large_file(true);
    let mut total = 0u64;
    let mut file_count = 0;
    for (idx, session) in sessions.iter().enumerate() {
        progress(format!(
            "Export {}/{}: {}",
            idx + 1,
            sessions.len(),
            session.key()
        ));
        let current =
            agents::read_session(session.agent, &session.source_root, &session.source_path)?;
        ensure!(
            current.id == session.id && current.project == session.project,
            "Session changed since selection"
        );
        let mut packed = PackedSession {
            session: current,
            files: Vec::new(),
        };
        for path in agents::related_files(session)? {
            paths::no_symlinks(&path)?;
            let relative = paths::portable(path.strip_prefix(&session.source_root)?)?;
            let before = path.metadata()?;
            ensure!(
                before.len() <= MAX_FILE,
                "File exceeds 2 GiB: {}",
                path.display()
            );
            total = total.checked_add(before.len()).context("Size overflow")?;
            file_count += 1;
            ensure!(
                total <= MAX_TOTAL && file_count <= MAX_FILES,
                "Bundle exceeds size/file-count limits"
            );
            let entry = format!("agents/{}/{idx}/{relative}", session.agent);
            writer.start_file(&entry, options)?;
            let mut source = File::open(&path)?;
            let mut hash = Sha256::new();
            let mut bytes = 0u64;
            let mut buf = [0; 65536];
            loop {
                let n = source.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                ensure!(bytes <= MAX_FILE, "Source grew beyond file size limit");
                hash.update(&buf[..n]);
                writer.write_all(&buf[..n])?;
            }
            let after = path.metadata()?;
            ensure!(
                bytes == before.len()
                    && before.len() == after.len()
                    && before.modified()? == after.modified()?,
                "Source changed during export: {}; stop the agent and retry",
                path.display()
            );
            let digest = format!("{:x}", hash.finalize());
            ensure!(hash_file(&path)? == digest, "Source changed during export");
            packed.files.push(Payload {
                entry,
                relative_path: relative,
                bytes,
                sha256: digest,
                transcript: path == session.source_path
                    || (session.agent == Agent::ClaudeCode
                        && path.extension().is_some_and(|e| e == "jsonl")
                        && path.components().any(|c| c.as_os_str() == "subagents")),
            });
        }
        manifest.sessions.push(packed);
    }
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    ensure!(
        bytes.len() as u64 <= MAX_MANIFEST,
        "Manifest exceeds 16 MiB"
    );
    writer.start_file("manifest.json", options)?;
    writer.write_all(&bytes)?;
    writer.finish()?;
    temp.as_file().sync_all()?;
    // Verify the completed snapshot before publishing it as a successful export.
    drop(open(temp.path(), progress)?);
    temp.persist_noclobber(&output).map_err(|e| e.error)?;
    Ok(manifest)
}

pub struct Bundle {
    pub manifest: Manifest,
    pub staging: TempDir,
}
impl Bundle {
    pub fn file(&self, p: &Payload) -> PathBuf {
        self.staging.path().join(&p.entry)
    }
}

/// Validate ALL payloads before import planning. ZIP filenames never become trusted paths.
pub fn open(path: &Path, progress: &dyn Fn(String)) -> Result<Bundle> {
    let mut zip = ZipArchive::new(File::open(path)?)?;
    ensure!(zip.len() <= MAX_FILES + 1, "Too many ZIP entries");
    let mut names = BTreeSet::new();
    let mut actual_total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i)?;
        paths::safe_relative(f.name())?;
        ensure!(
            names.insert(f.name().to_ascii_lowercase()),
            "Duplicate or case-colliding ZIP entry"
        );
        ensure!(
            !f.is_dir() && !f.is_symlink(),
            "Only regular ZIP files are allowed"
        );
        ensure!(f.size() <= MAX_FILE, "ZIP entry too large");
        actual_total = actual_total
            .checked_add(f.size())
            .context("Size overflow")?;
        ensure!(
            actual_total <= MAX_TOTAL + MAX_MANIFEST,
            "ZIP uncompressed size limit exceeded"
        );
    }
    let manifest: Manifest = {
        let f = zip.by_name("manifest.json")?;
        ensure!(f.size() <= MAX_MANIFEST, "Manifest too large");
        serde_json::from_reader(f.take(MAX_MANIFEST + 1))?
    };
    ensure!(
        manifest.format == "agentmoving" && manifest.version == 1,
        "Unsupported bundle format/version"
    );
    ensure!(!manifest.sessions.is_empty(), "Empty bundle");
    let staging = tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)?;
    let mut expected = BTreeSet::from(["manifest.json".to_owned()]);
    let mut keys = BTreeSet::new();
    let mut total = 0u64;
    for (idx, packed) in manifest.sessions.iter().enumerate() {
        progress(format!(
            "Verify {}/{}: {}",
            idx + 1,
            manifest.sessions.len(),
            packed.session.key()
        ));
        ensure!(
            keys.insert(packed.session.key()),
            "Duplicate session ID in manifest"
        );
        paths::safe_relative(&packed.session.id)?;
        ensure!(!packed.session.id.contains('/'), "Invalid session ID");
        ensure!(
            packed
                .files
                .iter()
                .filter(|p| p.relative_path == packed.session.relative_path && p.transcript)
                .count()
                == 1,
            "Missing/duplicate main transcript"
        );
        for p in &packed.files {
            let rel = paths::safe_relative(&p.entry)?;
            paths::safe_relative(&p.relative_path)?;
            ensure!(
                p.entry == format!("agents/{}/{idx}/{}", packed.session.agent, p.relative_path),
                "Unexpected payload location"
            );
            validate_owned(packed, p)?;
            ensure!(
                expected.insert(p.entry.to_ascii_lowercase()),
                "Repeated payload"
            );
            ensure!(
                p.bytes <= MAX_FILE
                    && p.sha256.len() == 64
                    && p.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid payload metadata"
            );
            total = total.checked_add(p.bytes).context("Size overflow")?;
            ensure!(total <= MAX_TOTAL, "Bundle too large");
            let f = zip.by_name(&p.entry)?;
            ensure!(f.size() == p.bytes, "ZIP size differs from manifest");
            let target = staging.path().join(rel);
            fs::create_dir_all(target.parent().unwrap())?;
            let mut out = File::create(&target)?;
            let n = std::io::copy(&mut f.take(p.bytes + 1), &mut out)?;
            ensure!(
                n == p.bytes && hash_file(&target)? == p.sha256,
                "Checksum mismatch: {}",
                p.entry
            );
            if p.transcript {
                agents::each_json(&target, |_| Ok(()))?;
            }
        }
        let main = packed
            .files
            .iter()
            .find(|p| p.relative_path == packed.session.relative_path)
            .unwrap();
        let extracted_root = staging
            .path()
            .join(format!("agents/{}/{idx}", packed.session.agent));
        let parsed = agents::read_session(
            packed.session.agent,
            &extracted_root,
            &staging.path().join(&main.entry),
        )?;
        ensure!(
            parsed.id == packed.session.id
                && parsed.project == packed.session.project
                && parsed.parent_session == packed.session.parent_session,
            "Manifest does not match native session metadata"
        );
    }
    ensure!(names == expected, "ZIP has unlisted or missing entries");
    Ok(Bundle { manifest, staging })
}

fn validate_owned(packed: &PackedSession, p: &Payload) -> Result<()> {
    let s = &packed.session;
    let main = &s.relative_path;
    let parts: Vec<_> = main.split('/').collect();
    match s.agent {
        Agent::Codex => ensure!(
            parts.len() >= 2
                && ["sessions", "archived_sessions"].contains(&parts[0])
                && main.ends_with(".jsonl")
                && main.ends_with(&format!("-{}.jsonl", s.id))
                && p.relative_path == *main
                && p.transcript,
            "Invalid Codex payload"
        ),
        Agent::ClaudeCode => {
            ensure!(
                parts.len() == 3 && parts[0] == "projects" && parts[2] == format!("{}.jsonl", s.id),
                "Invalid Claude transcript location"
            );
            ensure!(
                p.relative_path == *main
                    || p.relative_path
                        .starts_with(&format!("{}/", main.trim_end_matches(".jsonl")))
                    || p.relative_path
                        .starts_with(&format!("file-history/{}/", s.id)),
                "Claude dependency outside session scope"
            );
        }
        Agent::Pi => ensure!(
            (parts.len() == 1 || parts.len() == 2)
                && main.ends_with(".jsonl")
                && (p.relative_path == *main || p.relative_path == format!("{main}.acp.json")),
            "Invalid Pi payload"
        ),
    }
    Ok(())
}
