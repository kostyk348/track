//! track lib: лайфлог/трекер времени поверх eml-tag.
//!
//! Запись активности:
//!   X-Record-ID: rec_<nanos> | "current"
//!   X-Tag: <activity>
//!   X-Timestamp: <start, unix secs>
//!   тело: {"tag": ..., "start": ..., "end": null|..., "note": ...}
//!
//! Открытый интервал живёт в записи id="current"; stop() перезаписывает её.
//! Сводки: сумма end-start по тегам в диапазоне (header-scan + чтение тела
//! только у подходящих записей).

use emlbox::format::find_blank;
use emlbox::tagdb;
use serde_json::Value;
use std::path::Path;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn body_of(p: &Path) -> Result<Value, String> {
    let data = std::fs::read(p).map_err(|e| e.to_string())?;
    let (end, sep) = find_blank(&data).ok_or("no blank line in record")?;
    serde_json::from_slice(&data[end + sep..]).map_err(|e| e.to_string())
}

fn insert(db: &Path, id: &str, tag: &str, note: &str, start: u64, end: Option<u64>) -> Result<(), String> {
    let body = serde_json::json!({
        "tag": tag,
        "start": start,
        "end": end,
        "note": note,
    });
    tagdb::insert(db, id, &[tag.to_string()], "local", start, &body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Closed interval record.
pub fn log(db: &Path, tag: &str, note: &str, start: u64, end: Option<u64>) -> Result<String, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!("rec_{nanos}");
    insert(db, &id, tag, note, start, end)?;
    Ok(id)
}

/// Start an open interval (overwrites any previous open one).
pub fn now_interval(db: &Path, tag: &str, note: &str) -> Result<u64, String> {
    let start = now();
    insert(db, "current", tag, note, start, None)?;
    Ok(start)
}

/// Close the open interval, return its duration. None if no open interval.
pub fn stop(db: &Path) -> Result<Option<(String, u64, u64)>, String> {
    let p = db.join("current.eml");
    if !p.exists() {
        return Ok(None);
    }
    let b = body_of(&p)?;
    if b.get("end").and_then(|v| v.as_u64()).is_some() {
        return Ok(None); // already closed
    }
    let tag = b.get("tag").and_then(|v| v.as_str()).unwrap_or("?").to_string();
    let start = b.get("start").and_then(|v| v.as_u64()).unwrap_or(0);
    let note = b.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let end = now();
    insert(db, "current", &tag, &note, start, Some(end))?;
    Ok(Some((tag, start, end)))
}

/// Civil-date -> days since epoch (Howard Hinnant's algorithm, proleptic Gregorian).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse "YYYY-MM-DD" -> (start_epoch, end_epoch) in UTC.
pub fn day_bounds(date: &str) -> Result<(u64, u64), String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("bad date: {date} (need YYYY-MM-DD)"));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("bad year: {}", parts[0]))?;
    let m: i64 = parts[1].parse().map_err(|_| format!("bad month: {}", parts[1]))?;
    let d: i64 = parts[2].parse().map_err(|_| format!("bad day: {}", parts[2]))?;
    let start_day = days_from_civil(y, m, d);
    let start = start_day as u64 * 86400;
    Ok((start, start + 86400))
}

pub fn today_bounds() -> (u64, u64) {
    let days = (now() as i64) / 86400;
    let (y, m, d) = civil_from_days(days);
    day_bounds(&format!("{y:04}-{m:02}-{d:02}")).unwrap_or((0, u64::MAX))
}

/// Aggregate durations per tag in [from, to). Returns (tag, total_secs, sessions).
pub fn summary(db: &Path, from: u64, to: u64) -> Result<Vec<(String, u64, usize)>, String> {
    let q = format!("X-Timestamp >= {from} AND X-Timestamp < {to}");
    let (records, _) = tagdb::query(db, &q)?;
    let mut acc: std::collections::HashMap<String, (u64, usize)> = std::collections::HashMap::new();
    for (p, _) in records {
        let Ok(b) = body_of(&p) else { continue };
        let tag = b.get("tag").and_then(|v| v.as_str()).unwrap_or("?").to_string();
        if let (Some(s), Some(e)) = (b.get("start").and_then(|v| v.as_u64()), b.get("end").and_then(|v| v.as_u64())) {
            let dur = e.saturating_sub(s);
            let e = acc.entry(tag).or_insert((0, 0));
            e.0 += dur;
            e.1 += 1;
        }
    }
    let mut out: Vec<_> = acc.into_iter().collect();
    out.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    Ok(out
        .into_iter()
        .map(|(tag, (total, sessions))| (tag, total, sessions))
        .collect())
}

pub fn fmt_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup(tag: &str) -> PathBuf {
        let db = std::env::temp_dir().join(format!("track_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&db);
        std::fs::create_dir_all(&db).unwrap();
        db
    }

    #[test]
    fn civil_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 8, 12), days_from_civil(2024, 1, 1) + 954);
        let (y, m, d) = civil_from_days(0);
        assert_eq!((y, m, d), (1970, 1, 1));
        let (y, m, d) = civil_from_days(days_from_civil(2026, 8, 12));
        assert_eq!((y, m, d), (2026, 8, 12));
        assert_eq!(day_bounds("2026-08-12").unwrap(), (1786492800, 1786579200));
    }

    #[test]
    fn log_and_summary() {
        let db = setup("summary");
        log(&db, "work", "", 100, Some(200)).unwrap();
        log(&db, "work", "", 200, Some(350)).unwrap();
        log(&db, "rest", "", 350, Some(450)).unwrap();
        let s = summary(&db, 0, 1000).unwrap();
        assert_eq!(s, vec![
            ("work".to_string(), 250, 2),
            ("rest".to_string(), 100, 1),
        ]);
    }

    #[test]
    fn open_interval_now_stop() {
        let db = setup("open");
        let before = now();
        let start = now_interval(&db, "deep_work", "emlbox").unwrap();
        let after = now();
        assert!(start >= before && start <= after, "start must be now");
        let closed = stop(&db).unwrap();
        let (tag, s, e) = closed.unwrap();
        assert_eq!(tag, "deep_work");
        assert_eq!(s, start);
        assert!(e >= s);
        // second stop: no open interval
        assert!(stop(&db).unwrap().is_none());
    }

    #[test]
    fn day_filter_respects_bounds() {
        let db = setup("day");
        log(&db, "a", "", 100, Some(200)).unwrap();
        log(&db, "b", "", 86400, Some(86400 + 50)).unwrap();
        let (from, to) = day_bounds("1970-01-01").unwrap();
        assert_eq!(from, 0);
        let s = summary(&db, from, to).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "a");
    }

    #[test]
    fn fmt() {
        assert_eq!(fmt_duration(3725), "1h 2m");
        assert_eq!(fmt_duration(125), "2m 5s");
        assert_eq!(fmt_duration(9), "9s");
    }
}
