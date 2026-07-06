use crate::models::{AccountUsage, WindowUsage};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub struct ParsedEvent {
    pub kind: &'static str,
    pub msg_id: String,
    pub ts: String,
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub session_id: Option<String>,
}

pub fn model_multiplier(model: &str) -> f64 {
    let m = model.to_lowercase();
    if m.contains("haiku") {
        0.33
    } else if m.contains("opus") || m.contains("fable") || m.contains("mythos") {
        5.0
    } else {
        1.0
    }
}

/// Relative-cost units: roughly "sonnet input-token equivalents".
pub fn weighted_tokens(input: i64, output: i64, cache_read: i64, cache_write: i64, model: &str) -> f64 {
    (input as f64 + output as f64 * 5.0 + cache_read as f64 * 0.1 + cache_write as f64 * 1.25)
        * model_multiplier(model)
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

pub fn fmt_ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The account's real weekly-limit reset time, read from `<config>/.claude.json`. Claude
/// Code stores it under a GrowthBook flag whose experiment key can change, so we search
/// rather than hard-code the path. Rolled forward in 7-day steps to stay in the future.
pub fn plan_reset(config_dir: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let raw = fs::read_to_string(Path::new(config_dir).join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let s = find_plan_limits_end(&v)?;
    let mut r = parse_ts(&s)?;
    while r <= now {
        r += ChronoDuration::days(7);
    }
    Some(r)
}

pub struct LiveWindow {
    pub used_percentage: f64,
    pub resets_at: i64,
}

pub struct LiveUsage {
    pub five_hour: Option<LiveWindow>,
    pub seven_day: Option<LiveWindow>,
}

fn epoch_to_iso(secs: i64) -> Option<String> {
    DateTime::from_timestamp(secs, 0).map(fmt_ts)
}

/// Real usage from the raw status-line payload the tap saved to
/// `<config>/commander-statusline.json` (Claude Code's `rate_limits` object).
pub fn read_live_usage(config_dir: &str) -> Option<LiveUsage> {
    let raw = fs::read_to_string(Path::new(config_dir).join("commander-statusline.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let rl = v.get("rate_limits")?;
    let win = |key: &str| -> Option<LiveWindow> {
        let w = rl.get(key)?;
        Some(LiveWindow {
            used_percentage: w.get("used_percentage")?.as_f64()?,
            resets_at: w.get("resets_at")?.as_i64()?,
        })
    };
    let five_hour = win("five_hour");
    let seven_day = win("seven_day");
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(LiveUsage { five_hour, seven_day })
}

fn find_plan_limits_end(v: &Value) -> Option<String> {
    if let Some(s) = v.get("planLimitsEndDate").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    // nested under cachedGrowthBookFeatures.<experiment>.planLimitsEndDate
    if let Some(g) = v.get("cachedGrowthBookFeatures").and_then(|x| x.as_object()) {
        for val in g.values() {
            if let Some(s) = val.get("planLimitsEndDate").and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

pub fn parse_line(line: &str) -> Option<ParsedEvent> {
    if line.len() < 20 || !line.starts_with('{') {
        return None;
    }
    if !(line.contains("\"assistant\"") || line.contains("\"user\"")) {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = v.get("timestamp")?.as_str()?.to_string();
    let session_id = v.get("sessionId").and_then(|s| s.as_str()).map(|s| s.to_string());
    match v.get("type").and_then(|t| t.as_str())? {
        "assistant" => {
            let msg = v.get("message")?;
            let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("unknown").to_string();
            if model == "<synthetic>" {
                return None;
            }
            let usage = msg.get("usage")?;
            let gi = |k: &str| usage.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
            let (input, output) = (gi("input_tokens"), gi("output_tokens"));
            let (cache_read, cache_write) = (gi("cache_read_input_tokens"), gi("cache_creation_input_tokens"));
            if input + output + cache_read + cache_write == 0 {
                return None;
            }
            let msg_id = msg
                .get("id")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("uuid").and_then(|x| x.as_str()))?
                .to_string();
            Some(ParsedEvent { kind: "assistant", msg_id, ts, model, input, output, cache_read, cache_write, session_id })
        }
        "user" => {
            if v.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) {
                return None;
            }
            if v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
                return None;
            }
            let content = v.get("message")?.get("content")?;
            let is_prompt = match content {
                Value::String(s) => !s.trim().is_empty(),
                Value::Array(items) => {
                    items.iter().any(|i| i.get("type").and_then(|t| t.as_str()) == Some("text"))
                        && !items.iter().any(|i| i.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                }
                _ => false,
            };
            if !is_prompt {
                return None;
            }
            let msg_id = v.get("uuid")?.as_str()?.to_string();
            Some(ParsedEvent {
                kind: "user",
                msg_id,
                ts,
                model: String::new(),
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                session_id,
            })
        }
        _ => None,
    }
}

/// Incrementally scan one account's transcript files; only bytes appended since the
/// previous scan are read. Returns the number of new events stored.
pub fn scan_account(conn: &Connection, account_id: i64, config_dir: &str) -> Result<u64, String> {
    let projects_dir = Path::new(config_dir).join("projects");
    if !projects_dir.is_dir() {
        return Ok(0);
    }
    let cutoff = fmt_ts(Utc::now() - ChronoDuration::days(35));
    let mut inserted = 0u64;
    let proj_entries = fs::read_dir(&projects_dir).map_err(|e| e.to_string())?;
    for proj in proj_entries.flatten() {
        let pdir = proj.path();
        if !pdir.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&pdir) else { continue };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            let size = meta.len() as i64;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let key = path.to_string_lossy().to_string();
            let (mut offset, old_mtime): (i64, i64) = conn
                .query_row("SELECT bytes, mtime FROM scan_state WHERE path=?1", [&key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap_or((0, -1));
            if size == offset && mtime == old_mtime {
                continue;
            }
            if size < offset {
                offset = 0;
            }
            let Ok(mut file) = File::open(&path) else { continue };
            if file.seek(SeekFrom::Start(offset as u64)).is_err() {
                continue;
            }
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_err() {
                continue;
            }
            let tx = match conn.unchecked_transaction() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mut consumed = 0usize;
            let mut start = 0usize;
            while let Some(pos) = bytes[start..].iter().position(|&b| b == b'\n') {
                let end = start + pos;
                let raw = String::from_utf8_lossy(&bytes[start..end]);
                let line = raw.trim();
                if !line.is_empty() {
                    if let Some(ev) = parse_line(line) {
                        if ev.ts.as_str() >= cutoff.as_str() {
                            let w = if ev.kind == "assistant" {
                                weighted_tokens(ev.input, ev.output, ev.cache_read, ev.cache_write, &ev.model)
                            } else {
                                0.0
                            };
                            let n = tx
                                .execute(
                                    "INSERT OR IGNORE INTO usage_events(account_id,msg_id,kind,ts,model,input,output,cache_read,cache_write,weighted,session_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                                    params![account_id, ev.msg_id, ev.kind, ev.ts, ev.model, ev.input, ev.output, ev.cache_read, ev.cache_write, w, ev.session_id],
                                )
                                .unwrap_or(0);
                            inserted += n as u64;
                        }
                    }
                }
                start = end + 1;
                consumed = start;
            }
            let new_offset = offset + consumed as i64;
            let _ = tx.execute(
                "INSERT INTO scan_state(path,account_id,bytes,mtime) VALUES(?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET bytes=excluded.bytes, mtime=excluded.mtime",
                params![key, account_id, new_offset, mtime],
            );
            let _ = tx.commit();
        }
    }
    Ok(inserted)
}

pub struct WindowCalc {
    pub weighted: f64,
    pub prompts: i64,
    pub window_start: Option<DateTime<Utc>>,
}

/// Simulate Claude's 5-hour session windows: the first event opens a window, the first
/// event >= 5h later opens the next. Returns usage for the window containing `now`.
pub fn active_window(events: &[(DateTime<Utc>, f64, bool)], now: DateTime<Utc>) -> WindowCalc {
    let mut start: Option<DateTime<Utc>> = None;
    for (t, _, _) in events {
        match start {
            None => start = Some(*t),
            Some(s) if *t >= s + ChronoDuration::hours(5) => start = Some(*t),
            _ => {}
        }
    }
    if let Some(s) = start {
        if now < s + ChronoDuration::hours(5) {
            let mut weighted = 0.0;
            let mut prompts = 0i64;
            for (t, w, is_prompt) in events {
                if *t >= s {
                    weighted += w;
                    if *is_prompt {
                        prompts += 1;
                    }
                }
            }
            return WindowCalc { weighted, prompts, window_start: Some(s) };
        }
    }
    WindowCalc { weighted: 0.0, prompts: 0, window_start: None }
}

pub fn account_usage(
    conn: &Connection,
    account: &crate::models::Account,
    running_count: i64,
) -> Result<AccountUsage, String> {
    let now = Utc::now();
    let since16 = fmt_ts(now - ChronoDuration::hours(16));
    let mut stmt = conn
        .prepare("SELECT ts, weighted, kind FROM usage_events WHERE account_id=?1 AND ts>=?2 ORDER BY ts")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, f64, String)> = stmt
        .query_map(params![account.id, since16], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let events: Vec<(DateTime<Utc>, f64, bool)> = rows
        .into_iter()
        .filter_map(|(ts, w, k)| parse_ts(&ts).map(|t| (t, w, k == "user")))
        .collect();
    let calc = active_window(&events, now);
    let five_hour = WindowUsage {
        weighted: calc.weighted,
        prompts: calc.prompts,
        pct: if account.five_hour_budget > 0.0 {
            calc.weighted / account.five_hour_budget * 100.0
        } else {
            0.0
        },
        window_start: calc.window_start.map(fmt_ts),
        resets_at: calc.window_start.map(|s| fmt_ts(s + ChronoDuration::hours(5))),
        source: "estimate".into(),
    };

    // Anthropic weekly limits reset on a fixed 7-day cycle; use the real reset from
    // .claude.json when available, otherwise fall back to a rolling 7-day window.
    let reset = plan_reset(&account.config_dir, now);
    let week_start = reset.map(|r| r - ChronoDuration::days(7)).unwrap_or(now - ChronoDuration::days(7));
    let since_week = fmt_ts(week_start);
    let (w_weighted, w_prompts): (f64, i64) = conn
        .query_row(
            "SELECT coalesce(sum(weighted),0), coalesce(sum(CASE WHEN kind='user' THEN 1 ELSE 0 END),0) FROM usage_events WHERE account_id=?1 AND ts>=?2",
            params![account.id, since_week],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let weekly = WindowUsage {
        weighted: w_weighted,
        prompts: w_prompts,
        pct: if account.weekly_budget > 0.0 { w_weighted / account.weekly_budget * 100.0 } else { 0.0 },
        window_start: Some(fmt_ts(week_start)),
        resets_at: reset.map(fmt_ts),
        source: "estimate".into(),
    };

    // Prefer Claude Code's real rate-limit percentages when the status-line tap has
    // reported them for this account (overrides the estimate above).
    let mut five_hour = five_hour;
    let mut weekly = weekly;
    if let Some(live) = read_live_usage(&account.config_dir) {
        let now_epoch = now.timestamp();
        if let Some(w) = live.five_hour {
            if w.resets_at > now_epoch {
                five_hour.pct = w.used_percentage;
                five_hour.resets_at = epoch_to_iso(w.resets_at);
                five_hour.source = "live".into();
            }
        }
        if let Some(w) = live.seven_day {
            if w.resets_at > now_epoch {
                weekly.pct = w.used_percentage;
                weekly.resets_at = epoch_to_iso(w.resets_at);
                weekly.source = "live".into();
            }
        }
    }

    let last_active_at: Option<String> = conn
        .query_row("SELECT max(ts) FROM usage_events WHERE account_id=?1", [account.id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten();

    let cooling = account
        .limit_hit_until
        .as_deref()
        .and_then(parse_ts)
        .map(|t| t > now)
        .unwrap_or(false);
    let status = if !account.enabled {
        "disabled"
    } else if weekly.pct >= 100.0 {
        "limit_weekly"
    } else if cooling || five_hour.pct >= 100.0 {
        "limit_5h"
    } else if running_count > 0 {
        "busy"
    } else if five_hour.pct >= 80.0 || weekly.pct >= 85.0 {
        "near_limit"
    } else {
        "available"
    };

    let avg_per_prompt = if w_prompts > 0 {
        (w_weighted / w_prompts as f64).max(1000.0)
    } else {
        25_000.0
    };
    let rem5 = (account.five_hour_budget - five_hour.weighted).max(0.0);
    let rem_w = (account.weekly_budget - w_weighted).max(0.0);
    let est_remaining_prompts = Some((rem5.min(rem_w) / avg_per_prompt) as i64);

    let ev_count: i64 = conn
        .query_row("SELECT count(*) FROM usage_events WHERE account_id=?1", [account.id], |r| r.get(0))
        .unwrap_or(0);
    // real status-line data is authoritative → high confidence
    let live = five_hour.source == "live" || weekly.source == "live";
    let confidence = if live || (account.calibrated && ev_count > 200) {
        "high"
    } else if ev_count > 20 {
        "medium"
    } else {
        "low"
    };

    Ok(AccountUsage {
        account: account.clone(),
        status: status.to_string(),
        running_count,
        last_active_at,
        five_hour,
        weekly,
        est_remaining_prompts,
        confidence: confidence.to_string(),
    })
}

pub fn snapshot(conn: &Connection) -> Result<Vec<AccountUsage>, String> {
    let accounts = crate::accounts::all(conn)?;
    let mut counts: HashMap<i64, i64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT account_id, count(*) FROM instances WHERE status='running' GROUP BY account_id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            counts.insert(row.0, row.1);
        }
    }
    let mut out = Vec::new();
    for a in accounts {
        let rc = counts.get(&a.id).copied().unwrap_or(0);
        out.push(account_usage(conn, &a, rc)?);
    }
    Ok(out)
}

/// Largest weighted usage in any single 5-hour session window across the event list
/// (same window rule as `active_window`).
pub fn peak_5h_window(events: &[(DateTime<Utc>, f64, bool)]) -> f64 {
    let mut start: Option<DateTime<Utc>> = None;
    let mut cur = 0.0f64;
    let mut max = 0.0f64;
    for (t, w, _) in events {
        match start {
            None => {
                start = Some(*t);
                cur = *w;
            }
            Some(s) if *t >= s + ChronoDuration::hours(5) => {
                max = max.max(cur);
                start = Some(*t);
                cur = *w;
            }
            _ => cur += *w,
        }
    }
    max.max(cur)
}

/// Self-calibrate budgets from observed usage. There is no local source of truth for
/// Anthropic's real caps, so we treat each account's historical **peak** 5h-window and
/// peak 7-day volume (plus headroom) as the working budget. Budgets only grow — an exact
/// cap set by `calibrate_on_limit` (real limit) or a manual value in Settings is never
/// lowered. This keeps percentages meaningful and ranks accounts by real capacity.
pub fn ratchet_budgets(conn: &Connection, account_id: i64) -> Result<(), String> {
    let account = crate::accounts::get(conn, account_id)?;
    let now = Utc::now();
    let headroom = 1.2;

    // peak 5h window over the last 21 days
    let since21 = fmt_ts(now - ChronoDuration::days(21));
    let events: Vec<(DateTime<Utc>, f64, bool)> = {
        let mut stmt = conn
            .prepare("SELECT ts, weighted, kind FROM usage_events WHERE account_id=?1 AND ts>=?2 ORDER BY ts")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, f64, String)> = stmt
            .query_map(params![account_id, since21], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        rows.into_iter()
            .filter_map(|(ts, w, k)| parse_ts(&ts).map(|t| (t, w, k == "user")))
            .collect()
    };
    let peak_5h = peak_5h_window(&events);

    // peak rolling-7-day volume from daily buckets over the last 35 days
    let since35 = fmt_ts(now - ChronoDuration::days(35));
    let daily: Vec<(String, f64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT substr(ts,1,10) d, sum(weighted) FROM usage_events WHERE account_id=?1 AND ts>=?2 GROUP BY d ORDER BY d",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, f64)> = stmt
            .query_map(params![account_id, since35], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        rows
    };
    // A week holds ~33 five-hour windows; sustained heavy use fills a fraction of them.
    // Deriving the weekly budget from the (well-varied) 5h peak avoids the "current week ==
    // peak week" trap that pins every account at ~1/headroom before multi-week history
    // exists. The larger of the two estimates wins; a real weekly limit still overrides.
    const WEEKLY_TO_5H: f64 = 15.0;
    let peak_week = peak_7day(&daily).max(peak_5h * WEEKLY_TO_5H);

    let new_5h = (peak_5h * headroom).max(account.five_hour_budget);
    let new_wk = (peak_week * headroom).max(account.weekly_budget);
    // note: `calibrated` is intentionally left untouched — it marks budgets pinned by a
    // real observed limit (or manual entry), which this peak estimate is not.
    if new_5h > account.five_hour_budget * 1.001 || new_wk > account.weekly_budget * 1.001 {
        conn.execute(
            "UPDATE accounts SET five_hour_budget=?1, weekly_budget=?2 WHERE id=?3",
            params![new_5h.max(1.0), new_wk.max(1.0), account_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Max sum of any 7 consecutive calendar days given (date, weighted) daily buckets.
fn peak_7day(daily: &[(String, f64)]) -> f64 {
    use chrono::NaiveDate;
    let parsed: Vec<(NaiveDate, f64)> = daily
        .iter()
        .filter_map(|(d, w)| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(|nd| (nd, *w)))
        .collect();
    let mut max = 0.0;
    for (i, (start, _)) in parsed.iter().enumerate() {
        let mut sum = 0.0;
        for (d, w) in &parsed[i..] {
            if (*d - *start).num_days() >= 7 {
                break;
            }
            sum += w;
        }
        max = f64::max(max, sum);
    }
    max
}

/// When a real limit is observed, adopt the observed window usage as the budget.
pub fn calibrate_on_limit(conn: &Connection, account_id: i64, kind: &str) -> Result<(), String> {
    let account = crate::accounts::get(conn, account_id)?;
    let usage = account_usage(conn, &account, 0)?;
    let now = Utc::now();
    if kind == "weekly" {
        if usage.weekly.weighted > 10_000.0 {
            conn.execute(
                "UPDATE accounts SET weekly_budget=?1, calibrated=1 WHERE id=?2",
                params![usage.weekly.weighted, account_id],
            )
            .map_err(|e| e.to_string())?;
        }
    } else {
        let until = usage
            .five_hour
            .resets_at
            .clone()
            .unwrap_or_else(|| fmt_ts(now + ChronoDuration::minutes(60)));
        if usage.five_hour.weighted > 10_000.0 {
            conn.execute(
                "UPDATE accounts SET five_hour_budget=?1, calibrated=1, limit_hit_until=?2 WHERE id=?3",
                params![usage.five_hour.weighted, until, account_id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE accounts SET limit_hit_until=?1 WHERE id=?2",
                params![until, account_id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_simulation() {
        let t0 = Utc::now() - ChronoDuration::hours(7);
        let events = vec![
            (t0, 100.0, true),
            (t0 + ChronoDuration::hours(1), 200.0, false),
            (t0 + ChronoDuration::hours(6), 50.0, true),
        ];
        // now = t0+6.5h → active window started at t0+6h, contains only the third event
        let now = t0 + ChronoDuration::minutes(390);
        let calc = active_window(&events, now);
        assert_eq!(calc.window_start, Some(t0 + ChronoDuration::hours(6)));
        assert_eq!(calc.weighted, 50.0);
        assert_eq!(calc.prompts, 1);
        // now = t0+12h → no active window
        let calc2 = active_window(&events, t0 + ChronoDuration::hours(12));
        assert!(calc2.window_start.is_none());
        assert_eq!(calc2.weighted, 0.0);
    }

    #[test]
    fn parse_assistant_line() {
        let line = r#"{"type":"assistant","uuid":"u1","timestamp":"2026-07-05T10:00:00.000Z","sessionId":"s1","message":{"id":"msg_1","model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}}"#;
        let ev = parse_line(line).expect("should parse");
        assert_eq!(ev.kind, "assistant");
        assert_eq!(ev.msg_id, "msg_1");
        assert_eq!(ev.input, 10);
        assert_eq!(ev.output, 20);
        assert_eq!(ev.cache_read, 100);
        assert_eq!(ev.cache_write, 5);
    }

    #[test]
    fn parse_user_prompt_and_skip_tool_result() {
        let prompt = r#"{"type":"user","uuid":"u2","timestamp":"2026-07-05T10:00:00.000Z","sessionId":"s1","message":{"role":"user","content":"do the thing"}}"#;
        assert!(parse_line(prompt).is_some());
        let tool = r#"{"type":"user","uuid":"u3","timestamp":"2026-07-05T10:00:01.000Z","sessionId":"s1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        assert!(parse_line(tool).is_none());
        let meta = r#"{"type":"user","isMeta":true,"uuid":"u4","timestamp":"2026-07-05T10:00:02.000Z","message":{"role":"user","content":"meta"}}"#;
        assert!(parse_line(meta).is_none());
    }

    #[test]
    fn peak_windows() {
        let t0 = Utc::now() - ChronoDuration::days(10);
        // window A: 100+50 within 5h; window B (6h later): 400 alone → peak = 400
        let events = vec![
            (t0, 100.0, true),
            (t0 + ChronoDuration::hours(1), 50.0, false),
            (t0 + ChronoDuration::hours(6), 400.0, true),
        ];
        assert_eq!(peak_5h_window(&events), 400.0);

        let daily = vec![
            ("2026-06-01".to_string(), 10.0),
            ("2026-06-02".to_string(), 20.0),
            ("2026-06-09".to_string(), 100.0), // outside the first 7-day span
        ];
        // best 7-day span is [06-01..06-08): 10+20 = 30; the 06-09 day starts its own span = 100
        assert_eq!(peak_7day(&daily), 100.0);
    }

    #[test]
    fn weights() {
        assert_eq!(weighted_tokens(100, 0, 0, 0, "claude-sonnet-5"), 100.0);
        assert_eq!(weighted_tokens(0, 100, 0, 0, "claude-sonnet-5"), 500.0);
        assert!(weighted_tokens(100, 0, 0, 0, "claude-fable-5") > 400.0);
        assert!(weighted_tokens(100, 0, 0, 0, "claude-haiku-4-5") < 50.0);
    }
}
