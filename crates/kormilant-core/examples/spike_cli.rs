//! Terminal shell over kormilant-core, for testing the core without the GUI.
//!
//! cargo run -p kormilant-core --example spike_cli -- <repo> <slug> <ticket-file | --refine | --loop | --deliver> [model]
//!
//! While it runs, type on stdin:
//!   y / n          answer the oldest open permission request
//!   note <text>    send a steering note
//!   pause / resume freeze or continue the agent
//!   stop           kill the run

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use kormilant_core::{LoopConfig, Mode, RunConfig, RunEvent, start_loop, start_single};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, slug, ticket_file, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: spike_cli <repo> <slug> <ticket-file | --refine | --loop | --deliver> [model] [--base=REF]");
    };
    let base = rest.iter().find_map(|a| a.strip_prefix("--base=")).map(String::from);
    let model = rest.iter().find(|a| !a.starts_with("--")).cloned();
    if ticket_file == "--deliver" {
        let repo = std::path::Path::new(repo);
        let policy = kormilant_core::policy::Policy::load(repo)?;
        let plan = kormilant_core::delivery::plan(repo, slug, &policy)?;
        println!("{}", serde_json::to_string_pretty(&plan)?);
        println!("{:?}", kormilant_core::delivery::execute(repo, &plan)?);
        return Ok(());
    }

    let open_asks = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<()>();
    let asks = open_asks.clone();
    let sink = move |e: RunEvent| {
        match &e {
            RunEvent::PermissionAsk { request_id, tool, summary } => {
                asks.lock().unwrap().push_back(request_id.clone());
                println!("?? {tool}: {summary}   -> allow? [y/n]");
                return;
            }
            RunEvent::PermissionAnswered { request_id, .. } => asks.lock().unwrap().retain(|r| r != request_id),
            RunEvent::SessionEnded { .. } => {
                let _ = done_tx.send(());
            }
            _ => {}
        }
        println!("{}", serde_json::to_string(&e).unwrap_or_default());
    };
    let handle = match ticket_file.as_str() {
        "--loop" => {
            let mut cfg = LoopConfig::new(repo, slug);
            cfg.model = model;
            cfg.base = base;
            start_loop(cfg, sink).await?
        }
        other => {
            let mode = match other {
                "--refine" => Mode::Refine,
                file => Mode::Analyze { ticket: std::fs::read_to_string(file)? },
            };
            let mut cfg = RunConfig::new(repo, slug, mode);
            cfg.model = model;
            start_single(cfg, sink).await?
        }
    };
    eprintln!("session started");

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    loop {
        tokio::select! {
            _ = done_rx.recv() => break,
            line = stdin.next_line(), if stdin_open => {
                let Ok(Some(line)) = line else {
                    // stdin closed: keep running unattended until the session ends.
                    stdin_open = false;
                    continue;
                };
                let line = line.trim();
                match line {
                    "y" | "n" => {
                        let next = open_asks.lock().unwrap().front().cloned();
                        match next {
                            Some(id) => { handle.answer(&id, line == "y"); }
                            None => eprintln!("no open permission request"),
                        }
                    }
                    "stop" => handle.stop(),
                    "pause" => handle.pause("paused from the terminal"),
                    "resume" => handle.resume(),
                    _ if line.starts_with("note ") => {
                        if let Err(err) = handle.note(&line[5..]).await { eprintln!("note failed: {err}"); }
                    }
                    "" => {}
                    _ => eprintln!("commands: y | n | note <text> | pause | resume | stop"),
                }
            }
        }
    }
    // process::exit skips kill_on_drop: make sure no agent outlives the CLI.
    handle.shutdown();
    // Tokio's stdin reader blocks runtime shutdown until the next line, so exit explicitly.
    std::process::exit(0)
}
