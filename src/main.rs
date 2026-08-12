//! track CLI — трекер времени/лайфлог.
//!
//!   track init <db>
//!   track log <db> --tag T [--note S] [--start N] [--end N]
//!   track now <db> --tag T [--note S]     (открыть интервал)
//!   track stop <db>                       (закрыть)
//!   track day <db> [YYYY-MM-DD]
//!   track week <db>
//!   track tag <db> T
//!   track report <db>

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(|s| s.as_str()) {
        Some("init") => match args.get(2) {
            Some(db) => match std::fs::create_dir_all(db) {
                Ok(()) => {
                    println!("init {}", db);
                    0
                }
                Err(e) => err(&e.to_string()),
            },
            None => err("init: need db dir"),
        },
        Some("log") => cmd_log(&args[2..]),
        Some("now") => cmd_now(&args[2..]),
        Some("stop") => cmd_stop(&args[2..]),
        Some("day") => cmd_day(&args[2..]),
        Some("week") => cmd_week(&args[2..]),
        Some("tag") => cmd_tag(&args[2..]),
        Some("report") => cmd_report(&args[2..]),
        _ => {
            eprintln!("track: см. usage в шапке исходника");
            2
        }
    };
    std::process::exit(code);
}

fn parse_flags(a: &[String], i: usize) -> (Vec<(String, String)>, usize) {
    let mut out = Vec::new();
    let mut j = i;
    while j + 1 < a.len() {
        if a[j].starts_with("--") {
            out.push((a[j].clone(), a[j + 1].clone()));
            j += 2;
        } else {
            break;
        }
    }
    (out, j)
}

fn flag(flags: &[(String, String)], name: &str) -> Option<String> {
    flags
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn cmd_log(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("log: need db"),
    };
    let (flags, _) = parse_flags(a, 1);
    let tag = flag(&flags, "--tag").unwrap_or_else(|| "misc".to_string());
    let note = flag(&flags, "--note").unwrap_or_default();
    let start: u64 = flag(&flags, "--start").and_then(|s| s.parse().ok()).unwrap_or_else(now);
    let end: Option<u64> = flag(&flags, "--end").and_then(|s| s.parse().ok());
    match track::log(&db, &tag, &note, start, end) {
        Ok(id) => {
            println!("logged {id} [{}] {} -> {:?}", tag, start, end);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_now(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("now: need db"),
    };
    let (flags, _) = parse_flags(a, 1);
    let tag = flag(&flags, "--tag").unwrap_or_else(|| "misc".to_string());
    let note = flag(&flags, "--note").unwrap_or_default();
    match track::now_interval(&db, &tag, &note) {
        Ok(start) => {
            println!("started [{tag}] at {start}");
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_stop(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("stop: need db"),
    };
    match track::stop(&db) {
        Ok(Some((tag, start, end))) => {
            println!("stopped [{tag}] {}s ({} -> {})", end - start, start, end);
            0
        }
        Ok(None) => {
            println!("no open interval");
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_day(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("day: need db"),
    };
    let date = a.get(1).cloned().unwrap_or_default();
    let (from, to) = if date.is_empty() {
        track::today_bounds()
    } else {
        match track::day_bounds(&date) {
            Ok(b) => b,
            Err(e) => return err(&e),
        }
    };
    print_summary(&db, from, to, if date.is_empty() { "today" } else { &date })
}

fn cmd_week(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("week: need db"),
    };
    let to = now();
    let from = to.saturating_sub(7 * 86400);
    print_summary(&db, from, to, "last 7 days")
}

fn cmd_tag(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("tag: need db"),
    };
    let tag = match a.get(1) {
        Some(t) => t.clone(),
        None => return err("tag: need tag name"),
    };
    match track::summary(&db, 0, u64::MAX) {
        Ok(s) => match s.iter().find(|(t, _, _)| t == &tag) {
            Some((_, total, n)) => {
                println!("[{tag}] total {}, {} session(s)", track::fmt_duration(*total), n);
                0
            }
            None => {
                println!("[{tag}] no data");
                0
            }
        },
        Err(e) => err(&e),
    }
}

fn cmd_report(a: &[String]) -> i32 {
    let db = match a.get(0).map(PathBuf::from) {
        Some(p) => p,
        None => return err("report: need db"),
    };
    match track::summary(&db, 0, u64::MAX) {
        Ok(s) => {
            let total: u64 = s.iter().map(|(_, t, _)| t).sum();
            println!("=== report (all time) ===");
            for (tag, secs, n) in &s {
                println!("  {:>12}  {}  ({} sessions)", track::fmt_duration(*secs), tag, n);
            }
            println!("  total: {}", track::fmt_duration(total));
            0
        }
        Err(e) => err(&e),
    }
}

fn print_summary(db: &std::path::Path, from: u64, to: u64, label: &str) -> i32 {
    match track::summary(db, from, to) {
        Ok(s) => {
            let total: u64 = s.iter().map(|(_, t, _)| t).sum();
            println!("=== {label} ===");
            for (tag, secs, n) in &s {
                println!("  {:>12}  {}  ({} sessions)", track::fmt_duration(*secs), tag, n);
            }
            println!("  total: {}", track::fmt_duration(total));
            0
        }
        Err(e) => err(&e),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn err(e: &str) -> i32 {
    eprintln!("track: {e}");
    1
}
