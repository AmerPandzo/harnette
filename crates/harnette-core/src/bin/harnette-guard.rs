//! PreToolUse hook for Claude Code: `harnette-guard --repo <path> --role <role> [--slug <task>] [--read-dir <path>]… [--write-dir <path>]…`.
//! Reads the hook JSON on stdin and prints the policy decision. Never fails open.

use std::io::Read;
use std::path::PathBuf;

use harnette_core::policy::{Role, RunDirs, hook_response};

fn main() {
    let mut repo = None;
    let mut role = Role::Analyst;
    let mut slug = None;
    let mut dirs = RunDirs::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from),
            "--slug" => slug = args.next(),
            "--read-dir" => dirs.read.extend(args.next().map(PathBuf::from)),
            "--write-dir" => dirs.write.extend(args.next().map(PathBuf::from)),
            "--role" => role = args.next().and_then(|r| r.parse().ok()).unwrap_or(Role::Analyst),
            _ => {}
        }
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    // Without a repo there is nothing to check against: hand the call to the human.
    let repo = repo.unwrap_or_else(|| PathBuf::from("/nonexistent"));
    println!("{}", hook_response(&repo, role, slug.as_deref(), &dirs, &input));
}
