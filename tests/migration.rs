use agentmoving::{
    agents, archive,
    import::{self, ImportOptions},
    model::*,
    paths,
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn temp() -> tempfile::TempDir {
    #[cfg(windows)]
    let root = std::env::temp_dir();
    #[cfg(not(windows))]
    let root = std::env::temp_dir().canonicalize().unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn write(path: &Path, data: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}
fn fixture(root: &Path, agent: Agent, id: &str, project: &str) -> PathBuf {
    let (rel, records) = match agent {
        Agent::Codex => (
            format!("sessions/2026/09/21/rollout-2026-09-21T00-00-00-{id}.jsonl"),
            vec![
                json!({"type":"session_meta","timestamp":"2026-09-21T00:00:00Z","payload":{"id":id,"cwd":project,"timestamp":"2026-09-21T00:00:00Z","cli_version":"0.155.1","originator":"codex_cli_rs","source":"cli","model_provider":"openai"}}),
                json!({"type":"event_msg","timestamp":"2026-09-21T00:00:01Z","payload":{"type":"user_message","message":"hello"}}),
                json!({"type":"turn_context","payload":{"cwd":project},"future":{"keep":true}}),
            ],
        ),
        Agent::ClaudeCode => (
            format!(
                "projects/{}/{id}.jsonl",
                paths::claude_project(project).unwrap()
            ),
            vec![
                json!({"type":"user","sessionId":id,"cwd":project,"timestamp":"2026-09-21T00:00:00Z","version":"2.1.0","uuid":"m1","parentUuid":null,"message":{"role":"user","content":format!("do not rewrite {project}")}}),
                json!({"type":"assistant","sessionId":id,"cwd":project,"uuid":"m2","parentUuid":"m1","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}),
            ],
        ),
        Agent::Pi => (
            format!(
                "{}/2026-09-21T00-00-00_{id}.jsonl",
                paths::pi_project(project)
            ),
            vec![
                json!({"type":"session","version":3,"id":id,"cwd":project,"timestamp":"2026-09-21T00:00:00Z"}),
                json!({"type":"message","id":"m1","parentId":null,"message":{"role":"user","content":format!("do not rewrite {project}")}}),
            ],
        ),
    };
    let path = root.join(rel);
    write(
        &path,
        &records
            .iter()
            .map(|v| v.to_string() + "\n")
            .collect::<String>(),
    );
    path
}
fn scan(root: &Path, agent: Agent) -> Scan {
    agents::scan(vec![AgentRoot {
        agent,
        path: root.to_owned(),
        executable: None,
    }])
}

#[test]
fn three_agents_roundtrip_mapping_repeat_and_rollback() {
    for agent in Agent::ALL {
        let tmp = temp();
        let source = tmp.path().join("source");
        let dest = tmp.path().join("dest");
        let project = tmp.path().join("new-project");
        fs::create_dir(&project).unwrap();
        let file = fixture(&source, agent, "test-session", "C:\\old\\project");
        if agent == Agent::ClaudeCode {
            write(
                &file.with_extension("").join("subagents/agent-child.jsonl"),
                "{\"type\":\"user\",\"cwd\":\"C:\\\\old\\\\project\",\"message\":{\"content\":\"child\"}}\n",
            );
            write(
                &source.join("file-history/test-session/snapshot@v1"),
                "file snapshot",
            );
        }
        let scanned = scan(&source, agent);
        assert!(scanned.warnings.is_empty(), "{:?}", scanned.warnings);
        assert_eq!(scanned.sessions.len(), 1);
        let zip = tmp.path().join("test.zip");
        archive::export(&scanned.sessions, &scanned.sessions, &zip, &|_| {}).unwrap();
        let bundle = archive::open(&zip, &|_| {}).unwrap();
        let opts = ImportOptions {
            roots: [(agent, dest.clone())].into(),
            maps: vec![("C:\\old\\project".into(), project.display().to_string())],
            ..Default::default()
        };
        let plan = import::plan(&bundle, &opts, &|_| {}).unwrap();
        assert!(!dest.exists(), "dry-run writes target");
        assert_eq!(plan.sessions[0].status, "new");
        let receipt = import::apply(&plan, &opts, &|_| {}).unwrap();
        let imported = scan(&dest, agent);
        assert_eq!(imported.sessions.len(), 1);
        assert_eq!(imported.sessions[0].project, project.display().to_string());
        let text = fs::read_to_string(&imported.sessions[0].source_path).unwrap();
        if agent != Agent::Codex {
            assert!(text.contains("do not rewrite C:\\\\old\\\\project"));
        } else {
            assert!(text.contains("\"keep\":true"));
        }
        let again = import::plan(&bundle, &opts, &|_| {}).unwrap();
        assert_eq!(again.sessions[0].status, "identical");
        assert_eq!(import::rollback(&receipt).unwrap().status, "rolled-back");
        assert!(scan(&dest, agent).sessions.is_empty());
        assert!(file.exists());
    }
}
#[test]
fn rollback_preserves_modified_imports() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    fixture(&src, Agent::Pi, "one", "/missing");
    let s = scan(&src, Agent::Pi);
    let zip = t.path().join("a.zip");
    archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    let o = ImportOptions {
        roots: [(Agent::Pi, dst)].into(),
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    let receipt = import::apply(&p, &o, &|_| {}).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&p.sessions[0].files[0].target)
        .unwrap()
        .write_all(b"{}\n")
        .unwrap();
    assert!(import::rollback(&receipt).is_err());
    assert!(p.sessions[0].files[0].target.exists());
}
#[test]
fn conflicts_are_not_overwritten_and_missing_projects_block() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    fixture(&src, Agent::Codex, "one", "/missing");
    let s = scan(&src, Agent::Codex);
    let zip = t.path().join("a.zip");
    archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    let mut o = ImportOptions {
        roots: [(Agent::Codex, dst.clone())].into(),
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    assert_eq!(p.sessions[0].status, "blocked");
    assert!(import::apply(&p, &o, &|_| {}).is_err());
    assert!(!dst.exists());
    o.allow_missing_projects = true;
    let existing = fixture(&dst, Agent::Codex, "one", "/another");
    let before = fs::read(&existing).unwrap();
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    assert_eq!(p.sessions[0].status, "conflict");
    assert!(import::apply(&p, &o, &|_| {}).is_err());
    assert_eq!(fs::read(existing).unwrap(), before);
}
#[test]
fn pi_parent_is_exported_and_relinked() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    let parent = fixture(&src, Agent::Pi, "parent", "/old");
    let child = fixture(&src, Agent::Pi, "child", "/old");
    let text = fs::read_to_string(&child).unwrap();
    let mut lines = text.lines();
    let mut header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    header["parentSession"] = parent.display().to_string().into();
    write(
        &child,
        &format!("{}\n{}\n", header, lines.collect::<Vec<_>>().join("\n")),
    );
    let s = scan(&src, Agent::Pi);
    let chosen = s.sessions.iter().find(|s| s.id == "child").unwrap().clone();
    let zip = t.path().join("a.zip");
    let m = archive::export(&[chosen], &s.sessions, &zip, &|_| {}).unwrap();
    assert_eq!(m.sessions.len(), 2);
    let b = archive::open(&zip, &|_| {}).unwrap();
    let o = ImportOptions {
        roots: [(Agent::Pi, dst.clone())].into(),
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    import::apply(&p, &o, &|_| {}).unwrap();
    let s = scan(&dst, Agent::Pi);
    let c = s.sessions.iter().find(|s| s.id == "child").unwrap();
    assert!(Path::new(c.parent_session.as_ref().unwrap()).is_file());
}
#[test]
fn portable_path_validation_and_mapping() {
    for bad in [
        "../escape",
        "/abs",
        "C:/abs",
        "a\\b",
        "a/../b",
        "a//b",
        "CON.txt",
        "x/NUL",
        "x:ads",
        "a/./b",
        "a/",
        "a\0b",
    ] {
        assert!(paths::safe_relative(bad).is_err(), "{bad}");
    }
    let maps = vec![
        ("/old".into(), "D:/new".into()),
        ("/old/nested".into(), "E:/specific".into()),
    ];
    assert_eq!(
        paths::map_project("/old/nested/repo", &maps),
        "E:/specific/repo"
    );
    assert_eq!(paths::map_project("/older", &maps), "/older");
    assert_eq!(
        paths::pi_project("C:\\Users\\me\\repo"),
        "--C--Users-me-repo--"
    );
}
#[test]
fn malformed_sessions_reported_and_future_pi_rejected() {
    let t = temp();
    let f = fixture(t.path(), Agent::Pi, "one", "/old");
    write(
        &f,
        "{\"type\":\"session\",\"version\":99,\"id\":\"one\",\"cwd\":\"/old\"}\n",
    );
    let s = scan(t.path(), Agent::Pi);
    assert!(s.sessions.is_empty());
    assert_eq!(s.warnings.len(), 1);
    write(&f, "{unfinished");
    let s = scan(t.path(), Agent::Pi);
    assert_eq!(s.warnings.len(), 1);
}
fn rewrite_zip(path: &Path, change: impl Fn(&str, Vec<u8>) -> Vec<u8>) {
    let mut input = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut entries = Vec::new();
    for i in 0..input.len() {
        let mut f = input.by_index(i).unwrap();
        let name = f.name().to_owned();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut data).unwrap();
        entries.push((name.clone(), change(&name, data)));
    }
    drop(input);
    let mut out = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in entries {
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap();
}
#[test]
fn tampered_checksum_and_manifest_identity_rejected() {
    let t = temp();
    let src = t.path().join("s");
    fixture(&src, Agent::Pi, "one", "/old");
    let s = scan(&src, Agent::Pi);
    for mode in 0..2 {
        let zip = t.path().join(format!("{mode}.zip"));
        archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
        rewrite_zip(&zip, |name, mut data| {
            if mode == 0 && name != "manifest.json" {
                data[0] = b' ';
            }
            if mode == 1 && name == "manifest.json" {
                let mut v: serde_json::Value = serde_json::from_slice(&data).unwrap();
                v["sessions"][0]["session"]["id"] = "fake".into();
                return serde_json::to_vec(&v).unwrap();
            }
            data
        });
        assert!(archive::open(&zip, &|_| {}).is_err());
    }
}
#[test]
fn archive_path_traversal_rejected() {
    let t = temp();
    let path = t.path().join("bad.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file("../outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"bad").unwrap();
    zip.finish().unwrap();
    assert!(archive::open(&path, &|_| {}).is_err());
    assert!(!t.path().join("outside").exists());
}
#[cfg(unix)]
#[test]
fn symlink_targets_rejected() {
    let t = temp();
    let real = t.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(paths::no_symlinks(&link.join("x")).is_err());
}

#[test]
fn failed_second_file_rolls_back_first_and_preserves_existing() {
    use std::cell::Cell;
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    let file = fixture(&src, Agent::ClaudeCode, "one", "/old");
    write(
        &file.with_extension("").join("tool-results/result.txt"),
        "tool output",
    );
    let s = scan(&src, Agent::ClaudeCode);
    let zip = t.path().join("a.zip");
    archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    let o = ImportOptions {
        roots: [(Agent::ClaudeCode, dst.clone())].into(),
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    assert_eq!(p.sessions[0].files.len(), 2);
    write(&dst.join("keep.txt"), "existing");
    let count = Cell::new(0);
    let result = import::apply(&p, &o, &|_| {
        count.set(count.get() + 1);
        if count.get() == 2 {
            fs::remove_file(&p.sessions[0].files[1].source).unwrap();
        }
    });
    assert!(result.is_err());
    assert!(!p.sessions[0].files[0].target.exists());
    assert_eq!(
        fs::read_to_string(dst.join("keep.txt")).unwrap(),
        "existing"
    );
    assert!(!dst.join(".agentmoving.lock").exists());
}

#[test]
fn unchanged_mapping_keeps_original_bytes_and_scan_warnings() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    let f = fixture(&src, Agent::Pi, "one", "/old");
    let text = fs::read_to_string(&f).unwrap();
    write(&f, &format!(" \n{}\n", text.replace("\":", "\" : ")));
    let s = scan(&src, Agent::Pi);
    let zip = t.path().join("a.zip");
    archive::export_with_warnings(
        &s.sessions,
        &s.sessions,
        &zip,
        &["partial scan".into()],
        &|_| {},
    )
    .unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    assert!(b.manifest.warnings.contains(&"partial scan".into()));
    let o = ImportOptions {
        roots: [(Agent::Pi, dst)].into(),
        maps: vec![("/old".into(), "/old".into())],
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    assert_eq!(
        fs::read(&f).unwrap(),
        fs::read(&p.sessions[0].files[0].source).unwrap()
    );
}

#[test]
fn flat_pi_directory_and_sidecar_restore_to_project_group() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    let f = fixture(&src, Agent::Pi, "one", "/old");
    let flat = src.join("one.jsonl");
    fs::rename(&f, &flat).unwrap();
    write(&src.join("one.jsonl.acp.json"), "{\"extensionState\":true}");
    let s = scan(&src, Agent::Pi);
    assert_eq!(s.sessions.len(), 1);
    let zip = t.path().join("a.zip");
    archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    let o = ImportOptions {
        roots: [(Agent::Pi, dst.clone())].into(),
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    assert_eq!(p.sessions[0].files.len(), 2);
    let receipt = import::apply(&p, &o, &|_| {}).unwrap();
    assert!(dst.join("--old--/one.jsonl").is_file());
    assert!(dst.join("--old--/one.jsonl.acp.json").is_file());
    import::rollback(&receipt).unwrap();
}

#[test]
fn target_changed_after_plan_is_rejected_without_writes() {
    let t = temp();
    let src = t.path().join("s");
    let dst = t.path().join("d");
    fixture(&src, Agent::Codex, "one", "/old");
    let s = scan(&src, Agent::Codex);
    let zip = t.path().join("a.zip");
    archive::export(&s.sessions, &s.sessions, &zip, &|_| {}).unwrap();
    let b = archive::open(&zip, &|_| {}).unwrap();
    let o = ImportOptions {
        roots: [(Agent::Codex, dst)].into(),
        allow_missing_projects: true,
        ..Default::default()
    };
    let p = import::plan(&b, &o, &|_| {}).unwrap();
    write(&p.sessions[0].files[0].target, "existing race winner");
    assert!(import::apply(&p, &o, &|_| {}).is_err());
    assert_eq!(
        fs::read_to_string(&p.sessions[0].files[0].target).unwrap(),
        "existing race winner"
    );
}
