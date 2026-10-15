//! Harnette-owned build gate: agents' own test runs are not trusted, so the app runs the build itself.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use anyhow::{Context, bail};
use tokio::process::Command;
use tokio::sync::Notify;

#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub ok: bool,
    pub duration_ms: u64,
    pub tail: String,
    pub log: PathBuf,
}

/// Fail fast with an actionable message when a prerequisite is missing (e.g. Docker for Testcontainers).
pub fn check_requires(requires: &[String]) -> anyhow::Result<()> {
    for r in requires {
        let ok = match r.as_str() {
            "docker" => std::process::Command::new("docker")
                .arg("info")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success()),
            other => which(other)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success()),
        };
        if !ok {
            match r.as_str() {
                "docker" => bail!("Docker is not running (build.requires). Start OrbStack or Docker Desktop and run again."),
                other => bail!("`{other}` is not installed or not on PATH (build.requires)."),
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn which(tool: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("where");
    cmd.arg(tool);
    cmd
}

#[cfg(not(windows))]
fn which(tool: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", &format!("command -v {tool}")]);
    cmd
}

/// The platform shell running a build command: `sh -c`, or `cmd /C` on Windows.
fn shell(command: &str) -> Command {
    #[cfg(windows)]
    let cmd = {
        let mut cmd = Command::new("cmd");
        // raw_arg: cmd.exe does its own quote parsing, so the command must reach it unescaped.
        cmd.arg("/C").raw_arg(command);
        cmd
    };
    #[cfg(not(windows))]
    let cmd = {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    };
    cmd
}

pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Runs `command` with `sh -c` (`cmd /C` on Windows) in the repo, output to `log`. Cancelled builds count as failed.
pub async fn run_build(repo: &Path, command: &str, log: &Path, cancel: &Notify) -> anyhow::Result<BuildOutcome> {
    let file = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let started = Instant::now();
    let mut child = shell(command)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::from(file))
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start build `{command}`"))?;
    let ok = tokio::select! {
        status = child.wait() => status?.success(),
        _ = cancel.notified() => {
            let _ = child.kill().await;
            false
        }
    };
    let text = std::fs::read_to_string(log).unwrap_or_default();
    Ok(BuildOutcome { ok, duration_ms: started.elapsed().as_millis() as u64, tail: tail(&text, 40), log: log.to_path_buf() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_and_reports() {
        let dir = std::env::temp_dir();
        let log = dir.join(format!("harnette-build-{}.log", std::process::id()));
        let cancel = Notify::new();
        let green = run_build(&dir, "echo hello", &log, &cancel).await.unwrap();
        assert!(green.ok);
        assert_eq!(green.tail, "hello");
        let fail = if cfg!(windows) { "echo boom 1>&2 & exit 3" } else { "echo boom >&2; exit 3" };
        let red = run_build(&dir, fail, &log, &cancel).await.unwrap();
        assert!(!red.ok);
        assert!(red.tail.contains("boom"));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn requires_reports_missing_tools() {
        assert!(check_requires(&["git".into()]).is_ok());
        assert!(check_requires(&["definitely-not-a-tool-xyz".into()]).is_err());
    }
}
