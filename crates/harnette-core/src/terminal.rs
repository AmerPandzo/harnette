//! "Open in terminal": a login shell in the repo, or a Claude Code session resumed there, in one of a
//! fixed list of terminals. Only the repo path (quoted) and a checked session id reach the command line.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::editor::{Editor, app_path};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Launch {
    /// A new window of the app's binary: `-e <shell> -ilc <script>` (Stećak).
    Exec(&'static str),
    /// Terminal.app: `do script` over AppleScript.
    AppleTerminal,
}

struct Known {
    id: &'static str,
    name: &'static str,
    bundle_ids: &'static [&'static str],
    launch: Launch,
}

/// The first one installed is the default.
const KNOWN: &[Known] = &[
    Known { id: "stecak", name: "Stećak", bundle_ids: &["ba.stecak.terminal"], launch: Launch::Exec("stecak") },
    Known { id: "terminal", name: "Terminal", bundle_ids: &["com.apple.Terminal"], launch: Launch::AppleTerminal },
];

pub fn installed() -> Vec<Editor> {
    KNOWN.iter().filter(|k| app_path(k.bundle_ids).is_some()).map(|k| Editor { id: k.id.into(), name: k.name.into() }).collect()
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The shell script run in the new window: `cd` into the repo, then a shell, or `claude --resume` first.
fn script(repo: &Path, resume: Option<&str>) -> anyhow::Result<String> {
    let cd = format!("cd {}", sh_quote(&repo.display().to_string()));
    Ok(match resume {
        None => format!("{cd} && exec \"$SHELL\" -il"),
        Some(id) if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
            // Back to a shell when Claude exits.
            format!("{cd} && claude --resume {id}; exec \"$SHELL\" -il")
        }
        Some(_) => bail!("invalid session id"),
    })
}

fn command_for(launch: Launch, app: &Path, shell: &str, script: &str) -> (PathBuf, Vec<String>) {
    match launch {
        Launch::Exec(bin) => (app.join("Contents/MacOS").join(bin), vec!["-e".into(), shell.into(), "-ilc".into(), script.into()]),
        Launch::AppleTerminal => {
            let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
            let osa = format!("tell application \"Terminal\"\nactivate\ndo script \"{escaped}\"\nend tell");
            ("/usr/bin/osascript".into(), vec!["-e".into(), osa])
        }
    }
}

/// Open `terminal` in `repo`, resuming the Claude Code session `resume` when given. Returns without waiting.
pub fn open(repo: &Path, terminal: &str, resume: Option<&str>) -> anyhow::Result<()> {
    let repo = repo.canonicalize().with_context(|| format!("not a directory: {}", repo.display()))?;
    let k = KNOWN.iter().find(|k| k.id == terminal).with_context(|| format!("unknown terminal {terminal:?}"))?;
    let app = app_path(k.bundle_ids).with_context(|| format!("{} is not installed", k.name))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let (program, args) = command_for(k.launch, &app, &shell, &script(&repo, resume)?);
    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(&repo)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {}", k.name))?;
    // osascript exits at once; a terminal window lives on. Reap either off-thread.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_quote_the_repo_and_check_the_session() {
        let repo = Path::new("/r/it's here");
        assert_eq!(script(repo, None).unwrap(), r#"cd '/r/it'\''s here' && exec "$SHELL" -il"#);
        assert_eq!(script(repo, Some("ab-12")).unwrap(), r#"cd '/r/it'\''s here' && claude --resume ab-12; exec "$SHELL" -il"#);
        assert!(script(repo, Some("x; rm -rf ~")).is_err());
        assert!(script(repo, Some("")).is_err());
    }

    #[test]
    fn commands_per_terminal() {
        let (p, a) = command_for(Launch::Exec("stecak"), Path::new("/Applications/Stećak.app"), "/bin/zsh", "cd '/r' && exec \"$SHELL\" -il");
        assert_eq!(p, Path::new("/Applications/Stećak.app/Contents/MacOS/stecak"));
        assert_eq!(a, ["-e", "/bin/zsh", "-ilc", "cd '/r' && exec \"$SHELL\" -il"]);
        let (p, a) = command_for(Launch::AppleTerminal, Path::new("/x"), "/bin/zsh", "cd '/r' && exec \"$SHELL\" -il");
        assert_eq!(p, Path::new("/usr/bin/osascript"));
        assert!(a[1].contains(r#"do script "cd '/r' && exec \"$SHELL\" -il""#), "{}", a[1]);
    }
}
