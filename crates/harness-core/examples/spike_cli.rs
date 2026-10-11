//! Terminal shell over harness-core, for testing the core without the GUI.
//!
//! cargo run -p harness-core --example spike_cli -- <repo> <slug> <ticket-file | --refine> [model]
//!
//! While it runs, type on stdin:
//!   y / n          answer the oldest open permission request
//!   note <text>    send a steering note
//!   stop           kill the run

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use harness_core::{Mode, RunConfig, RunEvent, start_run};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, slug, ticket_file, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: spike_cli <repo> <slug> <ticket-file | --refine> [model]");
    };
    let mode = match ticket_file.as_str() {
        "--refine" => Mode::Refine,
        file => Mode::Analyze { ticket: std::fs::read_to_string(file)? },
    };
    let mut cfg = RunConfig::new(repo, slug, mode);
    cfg.model = rest.first().cloned();

    let open_asks = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<()>();
    let asks = open_asks.clone();
    let handle = start_run(cfg, move |e| {
        match &e {
            RunEvent::PermissionAsk { request_id, tool, summary } => {
                asks.lock().unwrap().push_back(request_id.clone());
                println!("?? {tool}: {summary}   -> allow? [y/n]");
                return;
            }
            RunEvent::PermissionAnswered { request_id, .. } => asks.lock().unwrap().retain(|r| r != request_id),
            RunEvent::Exited { .. } => {
                let _ = done_tx.send(());
            }
            _ => {}
        }
        println!("{}", serde_json::to_string(&e).unwrap_or_default());
    })
    .await?;
    eprintln!("run {} started", handle.run_id());

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            _ = done_rx.recv() => break,
            line = stdin.next_line() => {
                let Ok(Some(line)) = line else { continue };
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
                    _ if line.starts_with("note ") => {
                        if let Err(err) = handle.note(&line[5..]).await { eprintln!("note failed: {err}"); }
                    }
                    "" => {}
                    _ => eprintln!("commands: y | n | note <text> | stop"),
                }
            }
        }
    }
    // Tokio's stdin reader blocks runtime shutdown until the next line, so exit explicitly.
    std::process::exit(0)
}
