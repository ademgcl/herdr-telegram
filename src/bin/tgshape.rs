//! `tgshape` — reformat LLM output for Telegram.
//!
//! The point of this binary is that it is NOT a second implementation.
//! It `#[path]`-includes the same `telegram/shape.rs` the bot uses when
//! it sends a final card, so what you pipe through the CLI is exactly
//! what the bot posts. A copy would drift, and drift here means the
//! thing you tested is not the thing that runs.
//!
//! That inclusion is why `shape.rs` has no `crate::` dependencies: it has
//! to compile as a standalone module too.

#[path = "../telegram/shape.rs"]
mod shape;

use std::io::Read;

const USAGE: &str = "\
tgshape — reformat text for Telegram

USAGE:
    tgshape [OPTIONS] [FILE]      FILE omitted or `-` reads stdin

OPTIONS:
    -h, --help          Show this message
    -V, --version       Show the version
        --max-para N    Paragraph chunk size before splitting (default 200)
        --json          Wrap the result so a caller can read it back

Reads stdin, writes the shaped text to stdout. Exits non-zero on empty
input, or if the output came back empty when the input was not.
";

struct Args {
    file: Option<String>,
    max_para: usize,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut file = None;
    let mut max_para = 200;
    let mut json = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("tgshape {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--json" => json = true,
            "--max-para" => {
                max_para = it
                    .next()
                    .ok_or("--max-para needs a number")?
                    .parse()
                    .map_err(|_| "--max-para needs a number")?;
                if max_para == 0 {
                    return Err("--max-para must be greater than zero".into());
                }
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other}"));
            }
            other => {
                if file.is_some() {
                    return Err("one file at a time".into());
                }
                file = Some(other.to_string());
            }
        }
    }
    Ok(Args {
        file,
        max_para,
        json,
    })
}

fn read_input(file: Option<&str>) -> Result<String, String> {
    match file {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| format!("stdin: {e}"))?;
            Ok(s)
        }
        Some(p) => std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}")),
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("tgshape: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let input = match read_input(args.file.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tgshape: {e}");
            std::process::exit(2);
        }
    };
    if input.trim().is_empty() {
        eprintln!("tgshape: empty input");
        std::process::exit(1);
    }
    let out = shape::shape_with(&input, args.max_para);
    // The contract: a formatter that drops content is worse than a long
    // message. Empty output for non-empty input is a bug, not a result.
    if out.trim().is_empty() {
        eprintln!("tgshape: shaped to nothing — refusing to emit an empty reply");
        std::process::exit(1);
    }
    if args.json {
        println!("{}", serde_json::json!({ "text": out }));
    } else {
        println!("{out}");
    }
}
