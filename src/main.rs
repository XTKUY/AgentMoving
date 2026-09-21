use agentmoving::{
    agents, archive,
    import::{self, ImportOptions},
    model::Agent,
    paths, tui,
};
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Offline native-session migration for Codex, Claude Code and Pi"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(clap::Args, Clone)]
struct Source {
    #[arg(long, value_enum)]
    agent: Option<Agent>,
    /// Config roots for Codex/Claude; session directories for Pi. Repeat for multiple roots.
    #[arg(long)]
    root: Vec<PathBuf>,
}
#[derive(Subcommand)]
enum Command {
    /// Interactive terminal wizard (also the default with no command).
    Tui {
        #[command(flatten)]
        source: Source,
    },
    /// Find installed tools and session files without modifying them.
    Scan {
        #[command(flatten)]
        source: Source,
        #[arg(long)]
        json: bool,
    },
    /// Export all or selected native sessions. Existing output is never overwritten.
    Export {
        #[command(flatten)]
        source: Source,
        #[arg(long, conflicts_with = "session")]
        all: bool,
        /// Exact ID or agent:id; repeat for multiple sessions.
        #[arg(long)]
        session: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Verify checksums and inspect the manifest without modifying agent data.
    Inspect {
        bundle: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Restore native sessions; stop target agents before applying.
    Import {
        bundle: PathBuf,
        /// agent=path; Pi expects a session directory. Repeat for multiple agents.
        #[arg(long, value_parser = pair)]
        target: Vec<(String, String)>,
        /// Source project prefix=destination project prefix. Longest prefix wins.
        #[arg(long = "map", value_parser = pair)]
        maps: Vec<(String, String)>,
        #[arg(long)]
        session: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        skip_conflicts: bool,
        /// Store sessions even if project code is not yet present.
        #[arg(long)]
        allow_missing_projects: bool,
        #[arg(long)]
        json: bool,
    },
    /// Remove only unchanged files created by an import. Never removes existing files.
    Rollback { receipt: PathBuf },
}
fn pair(s: &str) -> std::result::Result<(String, String), String> {
    let (a, b) = s.split_once('=').ok_or("Expected LEFT=RIGHT")?;
    if a.is_empty() || b.is_empty() {
        return Err("Both sides must be nonempty".into());
    }
    Ok((a.to_owned(), b.to_owned()))
}
fn progress(s: String) {
    eprintln!("{}", paths::clean_display(&s));
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {}", paths::clean_display(&format!("{error:#}")));
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    match Cli::parse().command {
        None => tui::run(None, Vec::new()),
        Some(Command::Tui { source }) => tui::run(source.agent, source.root),
        Some(Command::Scan { source, json }) => {
            let scan = agents::scan(agents::roots(source.agent, &source.root)?);
            if json {
                println!("{}", serde_json::to_string_pretty(&scan)?);
            } else {
                for r in &scan.roots {
                    println!(
                        "{} | executable={} | data={}",
                        r.agent,
                        r.executable.is_some(),
                        paths::clean_display(&r.path.display().to_string())
                    );
                }
                for s in &scan.sessions {
                    println!(
                        "{} | {} | {} | {}",
                        s.key(),
                        paths::clean_display(&s.timestamp),
                        paths::clean_display(&s.project),
                        s.title
                    );
                }
                for w in &scan.warnings {
                    progress(format!("Warning: {w}"));
                }
                println!(
                    "{} sessions; {} warnings",
                    scan.sessions.len(),
                    scan.warnings.len()
                );
            }
            Ok(())
        }
        Some(Command::Export {
            source,
            all,
            session,
            project,
            output,
        }) => {
            ensure!(
                all || !session.is_empty(),
                "Choose --all or repeat --session ID"
            );
            let scan = agents::scan(agents::roots(source.agent, &source.root)?);
            for w in &scan.warnings {
                progress(format!("Warning: {w}"));
            }
            let selected: Vec<_> = scan
                .sessions
                .iter()
                .filter(|s| {
                    (all || session.iter().any(|id| id == &s.id || id == &s.key()))
                        && project.as_ref().is_none_or(|p| s.project.contains(p))
                })
                .cloned()
                .collect();
            for id in &session {
                ensure!(
                    selected.iter().any(|s| &s.id == id || &s.key() == id),
                    "Requested session not found: {id}"
                );
            }
            let manifest = archive::export_with_warnings(
                &selected,
                &scan.sessions,
                &output,
                &scan.warnings,
                &progress,
            )?;
            println!(
                "Exported {} sessions to {}",
                manifest.sessions.len(),
                output.display()
            );
            for w in &manifest.warnings {
                progress(format!("Warning: {w}"));
            }
            Ok(())
        }
        Some(Command::Inspect { bundle, json }) => {
            let bundle = archive::open(&bundle, &progress)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&bundle.manifest)?);
            } else {
                for s in &bundle.manifest.sessions {
                    println!(
                        "{} | {} | {} files",
                        s.session.key(),
                        paths::clean_display(&s.session.title),
                        s.files.len()
                    );
                }
                println!("Verified {} sessions", bundle.manifest.sessions.len());
            }
            Ok(())
        }
        Some(Command::Import {
            bundle,
            target,
            maps,
            session,
            dry_run,
            skip_conflicts,
            allow_missing_projects,
            json,
        }) => {
            let bundle = archive::open(&bundle, &progress)?;
            let mut options = ImportOptions {
                maps,
                skip_conflicts,
                allow_missing_projects,
                ..ImportOptions::default()
            };
            for (a, p) in target {
                let agent = Agent::ALL
                    .into_iter()
                    .find(|x| x.label() == a)
                    .context("Target agent must be codex, claude-code or pi")?;
                ensure!(
                    options.roots.insert(agent, p.into()).is_none(),
                    "Duplicate target for agent"
                );
            }
            for id in session {
                let matches: BTreeSet<_> = bundle
                    .manifest
                    .sessions
                    .iter()
                    .filter(|s| s.session.id == id || s.session.key() == id)
                    .map(|s| s.session.key())
                    .collect();
                ensure!(!matches.is_empty(), "Requested session not in bundle: {id}");
                options.selected.extend(matches);
            }
            let plan = import::plan(&bundle, &options, &progress)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&plan.sessions)?);
            } else {
                for s in &plan.sessions {
                    println!(
                        "{}:{} | {} | {}",
                        s.agent,
                        s.id,
                        s.status,
                        paths::clean_display(&s.project)
                    );
                    for note in &s.notes {
                        println!("  {}", paths::clean_display(note));
                    }
                }
            }
            if !dry_run {
                if plan.sessions.iter().all(|s| s.status == "identical") {
                    progress("No changes: all sessions already imported".into());
                    return Ok(());
                }
                let receipt = import::apply(&plan, &options, &progress)?;
                progress(format!(
                    "Files imported and verified. Receipt: {}. Native resume has not been verified.",
                    receipt.display()
                ));
            }
            Ok(())
        }
        Some(Command::Rollback { receipt }) => {
            let result = import::rollback(&paths::absolute(&receipt)?)?;
            println!("{}", result.status);
            Ok(())
        }
    }
}
