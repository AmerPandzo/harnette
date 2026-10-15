//! "Open in editor": find the editors installed on this Mac and open a repo file at a line.
//! Only files inside the repo can be opened, and only through a fixed list of editors,
//! so the web UI cannot turn this into "run any command".

use anyhow::{Context, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Launch {
    /// The IDE's own command-line launcher in `Contents/MacOS/<name>`: `<name> --line N <file>`.
    JetBrains(&'static str),
    /// A URL the editor registers, `<scheme>://file/<path>:<line>`, handed to `open`.
    Url(&'static str),
}

struct Known {
    id: &'static str,
    name: &'static str,
    bundle_ids: &'static [&'static str],
    launch: Launch,
}

const KNOWN: &[Known] = &[
    Known { id: "idea", name: "IntelliJ IDEA", bundle_ids: &["com.jetbrains.intellij", "com.jetbrains.intellij.ce"], launch: Launch::JetBrains("idea") },
    Known { id: "vscode", name: "VS Code", bundle_ids: &["com.microsoft.VSCode"], launch: Launch::Url("vscode") },
    Known { id: "cursor", name: "Cursor", bundle_ids: &["com.todesktop.230313mzl4w4u92"], launch: Launch::Url("cursor") },
    Known { id: "zed", name: "Zed", bundle_ids: &["dev.zed.Zed"], launch: Launch::Url("zed") },
    Known { id: "webstorm", name: "WebStorm", bundle_ids: &["com.jetbrains.WebStorm"], launch: Launch::JetBrains("webstorm") },
    Known { id: "rustrover", name: "RustRover", bundle_ids: &["com.jetbrains.rustrover"], launch: Launch::JetBrains("rustrover") },
];

/// "system" opens the file with whatever app macOS associates with it (no line).
pub const SYSTEM: &str = "system";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Editor {
    pub id: String,
    pub name: String,
}

pub(crate) fn app_path(bundle_ids: &[&str]) -> Option<PathBuf> {
    bundle_ids.iter().find_map(|id| {
        let out = Command::new("/usr/bin/mdfind").arg(format!("kMDItemCFBundleIdentifier == '{id}'")).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).find(|p| p.is_dir())
    })
}

/// Editors found on this machine, then the system default.
pub fn installed() -> Vec<Editor> {
    let mut out: Vec<Editor> = KNOWN
        .iter()
        .filter(|k| app_path(k.bundle_ids).is_some())
        .map(|k| Editor { id: k.id.into(), name: k.name.into() })
        .collect();
    out.push(Editor { id: SYSTEM.into(), name: "Default app".into() });
    out
}

/// A repo-relative path, resolved and checked to stay inside the repo.
pub fn resolve_in_repo(repo: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let repo = repo.canonicalize().with_context(|| format!("not a directory: {}", repo.display()))?;
    let file = repo.join(rel).canonicalize().with_context(|| format!("{rel} does not exist"))?;
    if !file.starts_with(&repo) {
        bail!("{rel} is outside the repository");
    }
    Ok(file)
}

fn file_url(scheme: &str, file: &Path, line: Option<u32>) -> String {
    let mut path = String::new();
    for b in file.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => path.push(b as char),
            _ => path.push_str(&format!("%{b:02X}")),
        }
    }
    match line {
        Some(l) => format!("{scheme}://file{path}:{l}"),
        None => format!("{scheme}://file{path}"),
    }
}

/// The program and arguments that open `file` at `line` in `launch`.
fn command_for(launch: Launch, app: &Path, file: &Path, line: Option<u32>) -> (PathBuf, Vec<String>) {
    match launch {
        Launch::JetBrains(bin) => {
            let mut args = vec![];
            if let Some(l) = line {
                args.extend(["--line".to_string(), l.to_string()]);
            }
            args.push(file.display().to_string());
            (app.join("Contents/MacOS").join(bin), args)
        }
        Launch::Url(scheme) => ("/usr/bin/open".into(), vec![file_url(scheme, file, line)]),
    }
}

/// Open `rel` (relative to `repo`) in `editor` at `line` (1-based). Returns without waiting.
pub fn open(repo: &Path, rel: &str, line: Option<u32>, editor: &str) -> anyhow::Result<()> {
    let file = resolve_in_repo(repo, rel)?;
    let (program, args) = if editor == SYSTEM {
        (PathBuf::from(if cfg!(windows) { "explorer" } else { "/usr/bin/open" }), vec![file.display().to_string()])
    } else {
        let k = KNOWN.iter().find(|k| k.id == editor).with_context(|| format!("unknown editor {editor:?}"))?;
        let app = app_path(k.bundle_ids).with_context(|| format!("{} is not installed", k.name))?;
        command_for(k.launch, &app, &file, line)
    };
    let mut child = Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {}", program.display()))?;
    // JetBrains launchers hand the file to the running IDE and exit; reap them off-thread.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn commands_per_editor() {
        let app = Path::new("/Apps/IntelliJ IDEA.app");
        let file = Path::new("/r/my repo/src/a.rs");
        let (p, a) = command_for(Launch::JetBrains("idea"), app, file, Some(12));
        assert_eq!(p, Path::new("/Apps/IntelliJ IDEA.app/Contents/MacOS/idea"));
        assert_eq!(a, ["--line", "12", "/r/my repo/src/a.rs"]);
        let (_, a) = command_for(Launch::JetBrains("idea"), app, file, None);
        assert_eq!(a, ["/r/my repo/src/a.rs"]);
        let (p, a) = command_for(Launch::Url("vscode"), app, file, Some(3));
        assert_eq!(p, Path::new("/usr/bin/open"));
        assert_eq!(a, ["vscode://file/r/my%20repo/src/a.rs:3"]);
        assert_eq!(file_url("zed", Path::new("/r/a&b?.md"), None), "zed://file/r/a%26b%3F.md");
    }

    #[test]
    fn only_files_inside_the_repo() {
        let root = std::env::temp_dir().join(format!("harnette-editor-{}", std::process::id()));
        let repo = root.join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/a.rs"), "").unwrap();
        fs::write(root.join("secret.txt"), "").unwrap();
        assert!(resolve_in_repo(&repo, "src/a.rs").unwrap().ends_with("repo/src/a.rs"));
        assert!(resolve_in_repo(&repo, "../secret.txt").is_err());
        assert!(resolve_in_repo(&repo, "/etc/hosts").is_err());
        assert!(resolve_in_repo(&repo, "src/missing.rs").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("secret.txt"), repo.join("link")).unwrap();
            assert!(resolve_in_repo(&repo, "link").is_err(), "a symlink out of the repo is refused");
        }
        assert!(open(&repo, "src/a.rs", None, "notepad").is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
