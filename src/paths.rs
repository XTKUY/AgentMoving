//! Foreign paths are strings, never parsed using the host OS's Path semantics.
use anyhow::{Result, bail, ensure};
use std::path::{Path, PathBuf};

pub fn home() -> Result<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Home directory unavailable; specify --root"))
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    ensure!(
        !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "Paths containing '..' are not supported: {}",
        path.display()
    );
    Ok(path)
}
/// Reject ambiguous Windows names on every OS so bundles have identical meaning.
pub fn safe_relative(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && value.len() <= 4096,
        "Invalid relative path length"
    );
    let mut result = PathBuf::new();
    for part in value.split('/') {
        ensure!(
            part.len() <= 255,
            "Path component exceeds 255 bytes: {value}"
        );
        ensure!(
            !part.is_empty() && part != "." && part != "..",
            "Unsafe path: {value}"
        );
        ensure!(
            !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c)),
            "Non-portable path: {value}"
        );
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            ![
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9"
            ]
            .contains(&stem.as_str()),
            "Reserved path: {value}"
        );
        result.push(part);
    }
    Ok(result)
}
pub fn portable(path: &Path) -> Result<String> {
    let parts: Result<Vec<_>> = path
        .components()
        .map(|c| match c {
            std::path::Component::Normal(s) => s
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path")),
            _ => bail!("Expected relative path"),
        })
        .collect();
    let s = parts?.join("/");
    safe_relative(&s)?;
    Ok(s)
}
/// Inspect every existing ancestor, including the root, before reads/writes.
pub fn no_symlinks(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m) => {
                ensure!(
                    !m.file_type().is_symlink(),
                    "Symlink is not allowed: {}",
                    ancestor.display()
                );
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        m.file_attributes() & 0x400 == 0,
                        "Windows reparse point is not allowed: {}",
                        ancestor.display()
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub fn normalize_foreign(s: &str) -> String {
    s.replace('\\', "/").trim_end_matches('/').to_owned()
}
pub fn map_project(project: &str, maps: &[(String, String)]) -> String {
    let p = normalize_foreign(project);
    let mut matches: Vec<_> = maps
        .iter()
        .filter_map(|(from, to)| {
            let f = normalize_foreign(from);
            if p == f {
                Some((f.len(), to.clone()))
            } else {
                p.strip_prefix(&(f.clone() + "/")).map(|suffix| {
                    (
                        f.len(),
                        format!("{}/{}", to.trim_end_matches(['/', '\\']), suffix),
                    )
                })
            }
        })
        .collect();
    matches.sort_by_key(|m| std::cmp::Reverse(m.0));
    matches
        .into_iter()
        .next()
        .map(|m| m.1)
        .unwrap_or_else(|| project.to_owned())
}
pub fn pi_project(s: &str) -> String {
    format!(
        "--{}--",
        s.trim_start_matches(['/', '\\'])
            .replace(['/', '\\', ':'], "-")
    )
}
pub fn claude_project(s: &str) -> Result<String> {
    let encoded: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    ensure!(
        encoded.len() <= 200,
        "Claude project encoding exceeds 200 characters; use a shorter destination path (version-specific hash encoding is not supported)"
    );
    Ok(encoded)
}
pub fn clean_display(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
pub fn clean_display_multiline(s: &str) -> String {
    s.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}
