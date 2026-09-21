//! Keyboard-driven wizard. Blocking filesystem work runs off the terminal event loop.
use crate::{
    agents, archive,
    import::{self, ImportOptions},
    model::*,
    paths,
};
use anyhow::{Result, bail, ensure};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{collections::BTreeSet, io::IsTerminal, path::PathBuf, sync::mpsc, time::Duration};

fn key() -> Result<Option<event::KeyEvent>> {
    if event::poll(Duration::from_millis(100))?
        && let Event::Key(k) = event::read()?
        && k.kind == KeyEventKind::Press
    {
        return Ok(Some(k));
    }
    Ok(None)
}
fn interrupted(k: event::KeyEvent) -> bool {
    k.code == KeyCode::Esc
        || k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL)
}
pub fn run(agent: Option<Agent>, roots: Vec<PathBuf>) -> Result<()> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "TUI requires an interactive terminal; use scan/export/import --help"
    );
    let mut terminal = ratatui::init();
    let result = wizard(&mut terminal, agent, roots);
    ratatui::restore();
    result
}
fn notice(t: &mut DefaultTerminal, title: &str, message: &str) -> Result<()> {
    let mut scroll = 0u16;
    loop {
        t.draw(|f| {
            f.render_widget(
                Paragraph::new(paths::clean_display_multiline(message))
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0))
                    .block(Block::bordered().title(format!(
                        "{} — ↑↓/PgDn 滚动 · Enter/Esc",
                        paths::clean_display(title)
                    ))),
                f.area(),
            )
        })?;
        if let Some(k) = key()? {
            if interrupted(k) || k.code == KeyCode::Enter {
                return Ok(());
            }
            match k.code {
                KeyCode::Down => scroll = scroll.saturating_add(1),
                KeyCode::Up => scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => scroll = scroll.saturating_add(10),
                KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                _ => {}
            }
        }
    }
}
fn input(t: &mut DefaultTerminal, title: &str, initial: &str) -> Result<Option<String>> {
    let mut value = initial.to_owned();
    loop {
        t.draw(|f| {
            let rows =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(f.area());
            f.render_widget(
                Paragraph::new(paths::clean_display(&value))
                    .block(Block::bordered().title(paths::clean_display(title))),
                rows[0],
            );
            f.render_widget(
                Paragraph::new("Enter 确认 · Esc 取消 · Ctrl+U 清空 · Backspace 删除"),
                rows[1],
            );
        })?;
        if let Some(k) = key()? {
            if interrupted(k) {
                return Ok(None);
            }
            match k.code {
                KeyCode::Enter => return Ok(Some(value)),
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => value.clear(),
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => value.push(c),
                _ => {}
            }
        }
    }
}
/// Search narrows the view; Space toggles a row, A toggles all visible rows.
fn choose(
    t: &mut DefaultTerminal,
    title: &str,
    rows: &[(String, String)],
    multi: bool,
) -> Result<Option<Vec<usize>>> {
    let mut marked = BTreeSet::new();
    let mut cursor = 0usize;
    let mut query = String::new();
    let mut searching = false;
    loop {
        let visible: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                format!("{} {}", r.0, r.1)
                    .to_lowercase()
                    .contains(&query.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect();
        cursor = cursor.min(visible.len().saturating_sub(1));
        t.draw(|f| {
            let sections = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(f.area());
            let columns = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(sections[0]);
            let items: Vec<_> = visible.iter().map(|i|ListItem::new(format!("{} {}", if marked.contains(i) { "[x]" } else { "[ ]" }, paths::clean_display(&rows[*i].0)))).collect();
            let list = List::new(items).block(Block::bordered().title(title)).highlight_style(Style::default().bg(Color::Blue));
            let mut state = ListState::default().with_selected(if visible.is_empty() { None } else { Some(cursor) });
            f.render_stateful_widget(list, columns[0], &mut state);
            let detail = visible.get(cursor).map(|i|paths::clean_display_multiline(&rows[*i].1)).unwrap_or_else(||"没有匹配项".into());
            f.render_widget(Paragraph::new(detail).wrap(Wrap { trim:false }).block(Block::bordered().title("详情")), columns[1]);
            f.render_widget(Paragraph::new(format!("↑↓ 移动 · Space 选择 · A 全选当前筛选 · Enter 确认 · Esc 返回\n/ 搜索：{query}{} · 已选 {}", if searching { "▏" } else { "" }, marked.len())),sections[1]);
        })?;
        if let Some(k) = key()? {
            if searching {
                match k.code {
                    KeyCode::Esc | KeyCode::Enter => searching = false,
                    KeyCode::Backspace => {
                        query.pop();
                        cursor = 0;
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        cursor = 0;
                    }
                    _ => {}
                }
                continue;
            }
            if interrupted(k) {
                return Ok(None);
            }
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    cursor = (cursor + 1).min(visible.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
                KeyCode::Char('/') => searching = true,
                KeyCode::Char(' ') if multi => {
                    if let Some(i) = visible.get(cursor)
                        && !marked.remove(i)
                    {
                        marked.insert(*i);
                    }
                }
                KeyCode::Char('a' | 'A') if multi => {
                    let clear = visible.iter().all(|i| marked.contains(i));
                    for i in &visible {
                        if clear {
                            marked.remove(i);
                        } else {
                            marked.insert(*i);
                        }
                    }
                }
                KeyCode::Enter => {
                    if !multi {
                        return Ok(visible.get(cursor).map(|i| vec![*i]));
                    }
                    if !marked.is_empty() {
                        return Ok(Some(marked.into_iter().collect()));
                    }
                }
                _ => {}
            }
        }
    }
}
fn job<T: Send + 'static>(
    t: &mut DefaultTerminal,
    title: &str,
    work: impl FnOnce(&dyn Fn(String)) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (status_tx, status_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = work(&|s| {
            let _ = status_tx.send(s);
        });
        let _ = result_tx.send(result);
    });
    let mut status = "处理中…".to_owned();
    loop {
        for s in status_rx.try_iter() {
            status = s;
        }
        match result_rx.try_recv() {
            Ok(r) => return r,
            Err(mpsc::TryRecvError::Disconnected) => bail!("Worker stopped unexpectedly"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        t.draw(|f| {
            f.render_widget(
                Paragraph::new(format!(
                    "{}\n\n操作中，请等待完成。",
                    paths::clean_display(&status)
                ))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(title)),
                f.area(),
            )
        })?;
        let _ = key()?;
    }
}
fn wizard(t: &mut DefaultTerminal, agent: Option<Agent>, roots: Vec<PathBuf>) -> Result<()> {
    loop {
        let menu = vec![
            (
                "导出会话".into(),
                "自动检测 Agent → 选择会话 → 保存 ZIP 迁移包".into(),
            ),
            (
                "导入会话".into(),
                "校验迁移包 → 选择会话 → 设置目标项目目录 → 查看计划 → 导入".into(),
            ),
        ];
        let Some(choice) = choose(t, "AgentMoving 0.1", &menu, false)? else {
            return Ok(());
        };
        let result = if choice[0] == 0 {
            export_wizard(t, agent, roots.clone())
        } else {
            import_wizard(t)
        };
        if let Err(e) = result {
            notice(t, "操作未完成", &format!("{e:#}"))?;
        }
    }
}
fn export_wizard(t: &mut DefaultTerminal, agent: Option<Agent>, roots: Vec<PathBuf>) -> Result<()> {
    let scanned = job(t, "检测 Agent", move |_| {
        Ok(agents::scan(agents::roots(agent, &roots)?))
    })?;
    let root_rows: Vec<_> = scanned
        .roots
        .iter()
        .map(|r| {
            (
                format!(
                    "{} · {} 个会话",
                    r.agent,
                    scanned
                        .sessions
                        .iter()
                        .filter(|s| s.source_root == r.path && s.agent == r.agent)
                        .count()
                ),
                format!(
                    "程序：{}\n数据目录：{}",
                    r.executable
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or("未发现".into()),
                    r.path.display()
                ),
            )
        })
        .collect();
    if !scanned.warnings.is_empty() {
        notice(t, "扫描警告", &scanned.warnings.join("\n"))?;
    }
    let Some(root_ids) = choose(t, "选择 Agent / 数据目录", &root_rows, true)? else {
        return Ok(());
    };
    let sessions: Vec<_> = scanned
        .sessions
        .iter()
        .filter(|s| {
            root_ids.iter().any(|i| {
                scanned.roots[*i].agent == s.agent && scanned.roots[*i].path == s.source_root
            })
        })
        .cloned()
        .collect();
    if sessions.is_empty() {
        return notice(
            t,
            "扫描结果",
            "没有可导出的有效会话。自定义目录可用 agentmoving tui --agent NAME --root PATH。",
        );
    }
    let rows: Vec<_> = sessions
        .iter()
        .map(|s| {
            (
                format!("{} · {}", s.agent, s.title),
                format!(
                    "{}\n项目：{}\n时间：{}\n大小：{} bytes\n归档：{}\n预览：{}",
                    s.id, s.project, s.timestamp, s.bytes, s.archived, s.title
                ),
            )
        })
        .collect();
    let Some(ids) = choose(t, "选择会话", &rows, true)? else {
        return Ok(());
    };
    let Some(path) = input(
        t,
        "导出文件路径",
        &format!(
            "agentmoving-{}.agentmove.zip",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ),
    )?
    else {
        return Ok(());
    };
    let selected = ids
        .into_iter()
        .map(|i| sessions[i].clone())
        .collect::<Vec<_>>();
    let output = PathBuf::from(path);
    let display = output.display().to_string();
    let manifest = job(t, "导出", move |p| {
        archive::export_with_warnings(&selected, &scanned.sessions, &output, &scanned.warnings, p)
    })?;
    notice(
        t,
        "导出完成",
        &format!(
            "{} 个会话\n文件：{display}\n{}",
            manifest.sessions.len(),
            manifest.warnings.join("\n")
        ),
    )
}
fn import_wizard(t: &mut DefaultTerminal) -> Result<()> {
    let Some(path) = input(t, "迁移包路径", "")? else {
        return Ok(());
    };
    let bundle = job(t, "校验迁移包", move |p| {
        archive::open(&PathBuf::from(path), p)
    })?;
    let rows: Vec<_> = bundle
        .manifest
        .sessions
        .iter()
        .map(|p| {
            (
                format!("{} · {}", p.session.agent, p.session.title),
                format!(
                    "{}\n{}\n{}",
                    p.session.id, p.session.project, p.session.timestamp
                ),
            )
        })
        .collect();
    let Some(ids) = choose(t, "选择导入会话（Pi 请一并选择父会话）", &rows, true)?
    else {
        return Ok(());
    };
    let mut options = ImportOptions::default();
    let mut agent_set = BTreeSet::new();
    let mut projects = BTreeSet::new();
    for i in ids {
        let s = &bundle.manifest.sessions[i].session;
        options.selected.insert(s.key());
        agent_set.insert(s.agent);
        projects.insert(s.project.clone());
    }
    for a in agent_set {
        let default = agents::default_root(a)?.display().to_string();
        let Some(root) = input(
            t,
            &format!("{a} 目标数据目录（Pi 为 sessions 目录）"),
            &default,
        )?
        else {
            return Ok(());
        };
        options.roots.insert(a, PathBuf::from(root));
    }
    for project in projects {
        let Some(dest) = input(t, &format!("项目目录映射：{project}"), &project)? else {
            return Ok(());
        };
        ensure!(!dest.is_empty(), "目标项目目录不能为空");
        options.maps.push((project, dest));
    }
    let opts = options.clone();
    let plan = job(t, "生成导入计划", move |p| {
        import::plan(&bundle, &opts, p)
    })?;
    let summary = plan
        .sessions
        .iter()
        .map(|s| {
            format!(
                "{}:{} [{}]\n目标项目：{}\n{}",
                s.agent,
                s.id,
                s.status,
                s.project,
                s.notes.join("\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    notice(t, "导入计划（尚未写入）", &summary)?;
    if plan
        .sessions
        .iter()
        .any(|s| s.status == "blocked" || s.status == "conflict")
    {
        return notice(
            t,
            "导入受阻",
            "请修正项目路径或会话冲突后重试。CLI 支持 --allow-missing-projects / --skip-conflicts。",
        );
    }
    if plan.sessions.iter().all(|s| s.status == "identical") {
        return notice(t, "无需导入", "所有会话已存在且内容一致。");
    }
    let Some(confirm) = input(t, "请关闭目标 Agent。输入 IMPORT 执行以上计划", "")?
    else {
        return Ok(());
    };
    if confirm != "IMPORT" {
        return Ok(());
    }
    let receipt = job(t, "导入", move |p| import::apply(&plan, &options, p))?;
    notice(
        t,
        "导入完成",
        &format!(
            "文件校验通过。\n回滚记录：{}\n目标 Agent 内的会话发现与继续聊天尚未验证，请重启 Agent 后检查。",
            receipt.display()
        ),
    )
}
