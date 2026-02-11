use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadMeta {
    pub channel_id: String,
    pub thread_ts: String,
    pub channel_name: String,
    pub workspace_url: String,
    pub followed_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadMessage {
    pub user: String,
    pub text: String,
    pub ts: String,
    pub thread_ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<ThreadReaction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ThreadAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadReaction {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadAttachment {
    pub name: String,
    pub mimetype: String,
    pub size: u64,
}

/// Parse a Slack thread URL into (workspace_url, channel_id, thread_ts).
///
/// Example: https://signalft.slack.com/archives/C02971LRE1Y/p1770737425723409
///   -> ("https://signalft.slack.com", "C02971LRE1Y", "1770737425.723409")
pub fn parse_thread_url(url: &str) -> Result<(String, String, String), String> {
    let url = url.trim();
    let path = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or("Invalid Slack URL: must start with https://")?;

    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if parts.len() < 2 {
        return Err("Invalid Slack URL format".to_string());
    }

    let host = parts[0];
    if !host.ends_with(".slack.com") {
        return Err("Not a Slack URL".to_string());
    }
    let workspace_url = format!("https://{}", host);

    let segments: Vec<&str> = parts[1].split('/').collect();
    if segments.len() < 3 || segments[0] != "archives" {
        return Err("Invalid Slack thread URL: expected /archives/<channel>/p<ts>".to_string());
    }

    let channel_id = segments[1].to_string();
    let ts_raw = segments[2];
    if !ts_raw.starts_with('p') || ts_raw.len() < 11 {
        return Err("Invalid thread timestamp in URL".to_string());
    }

    let digits = &ts_raw[1..]; // strip 'p'
    if digits.len() < 11 {
        return Err("Thread timestamp too short".to_string());
    }
    let thread_ts = format!("{}.{}", &digits[..10], &digits[10..]);

    Ok((workspace_url, channel_id, thread_ts))
}

fn slck_path() -> String {
    format!(
        "/opt/homebrew/bin:/usr/local/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Fetch channel name via slck channels get.
pub fn fetch_channel_name(channel_id: &str) -> Option<String> {
    let output = std::process::Command::new("slck")
        .args(["channels", "get", channel_id, "-o", "json"])
        .env("PATH", slck_path())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let resp: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    resp.get("data")
        .and_then(|d| d.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Fetch thread messages via slck.
pub fn fetch_thread_messages(
    channel_id: &str,
    thread_ts: &str,
    limit: u32,
) -> Result<Vec<ThreadMessage>, String> {
    let output = std::process::Command::new("slck")
        .args([
            "messages",
            "thread",
            channel_id,
            thread_ts,
            "--limit",
            &limit.to_string(),
            "-o",
            "json",
        ])
        .env("PATH", slck_path())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "slck not found. Install it first: brew install open-cli-collective/tap/slck"
                    .to_string()
            } else {
                format!("Error running slck: {}", e)
            }
        })?;

    if !output.status.success() {
        return Err(format!(
            "slck error: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("Error parsing slck output: {}", e))?;

    // slck wraps response in { "data": [...] }
    let messages_arr = if let Some(arr) = resp.get("data").and_then(|d| d.as_array()) {
        arr.clone()
    } else if let Some(arr) = resp.as_array() {
        arr.clone()
    } else {
        return Ok(vec![]);
    };

    let messages: Vec<ThreadMessage> = messages_arr
        .iter()
        .map(|msg| {
            let reactions = msg.get("reactions").and_then(|r| r.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        Some(ThreadReaction {
                            name: r.get("name")?.as_str()?.to_string(),
                            count: r.get("count")?.as_u64()? as u32,
                        })
                    })
                    .collect()
            });

            let files = msg.get("files").and_then(|f| f.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        Some(ThreadAttachment {
                            name: f
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            mimetype: f
                                .get("mimetype")
                                .and_then(|m| m.as_str())
                                .unwrap_or("application/octet-stream")
                                .to_string(),
                            size: f.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect()
            });

            ThreadMessage {
                user: msg
                    .get("user")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string(),
                text: msg
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                ts: msg
                    .get("ts")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                thread_ts: msg
                    .get("thread_ts")
                    .and_then(|t| t.as_str())
                    .unwrap_or(thread_ts)
                    .to_string(),
                reactions,
                files,
                reply_count: msg.get("reply_count").and_then(|r| r.as_u64()).map(|r| r as u32),
            }
        })
        .collect();

    Ok(messages)
}

/// Resolve user IDs to display names via slck.
pub fn resolve_user_names(user_ids: &[String]) -> HashMap<String, String> {
    let mut names = HashMap::new();

    for id in user_ids {
        if id.is_empty() {
            continue;
        }
        let output = std::process::Command::new("slck")
            .args(["users", "get", id, "-o", "json"])
            .env("PATH", slck_path())
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                if let Ok(resp) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    // slck wraps in { "data": { "real_name": "...", "profile": { "display_name": "..." } } }
                    let data = resp.get("data").unwrap_or(&resp);
                    let name = data
                        .get("real_name")
                        .or_else(|| data.get("name"))
                        .or_else(|| {
                            data.get("profile")
                                .and_then(|p| p.get("display_name").or_else(|| p.get("real_name")))
                        })
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());

                    if let Some(n) = name {
                        names.insert(id.clone(), n.to_string());
                    }
                }
            }
        }
    }

    names
}

/// Send a reply to a thread via slck.
pub fn send_thread_reply(
    channel_id: &str,
    thread_ts: &str,
    text: &str,
) -> Result<(), String> {
    let output = std::process::Command::new("slck")
        .args([
            "messages",
            "send",
            channel_id,
            text,
            "--thread",
            thread_ts,
            "--simple",
        ])
        .env("PATH", slck_path())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "slck not found. Install it first: brew install open-cli-collective/tap/slck"
                    .to_string()
            } else {
                format!("Error running slck: {}", e)
            }
        })?;

    if !output.status.success() {
        return Err(format!(
            "slck error: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

/// Read .twapp-thread.json from a directory.
pub fn read_thread_meta(dir: &Path) -> Option<ThreadMeta> {
    let path = dir.join(".twapp-thread.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write .twapp-thread.json to a directory.
pub fn write_thread_meta(dir: &Path, meta: &ThreadMeta) -> Result<(), String> {
    let path = dir.join(".twapp-thread.json");
    let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

// --- CLI command handlers ---

pub fn cmd_thread_follow(url: &str, dir: Option<&str>) -> i32 {
    let (workspace_url, channel_id, thread_ts) = match parse_thread_url(url) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let channel_name = fetch_channel_name(&channel_id).unwrap_or_else(|| channel_id.clone());

    let target_dir = resolve_dir(dir);

    let meta = ThreadMeta {
        channel_id,
        thread_ts,
        channel_name: channel_name.clone(),
        workspace_url,
        followed_at: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(e) = write_thread_meta(&target_dir, &meta) {
        eprintln!("Error writing thread file: {}", e);
        return 1;
    }

    println!("Following thread in #{}", channel_name);
    println!("Written to {}", target_dir.join(".twapp-thread.json").display());
    0
}

pub fn cmd_thread_unfollow(dir: Option<&str>) -> i32 {
    let target_dir = resolve_dir(dir);
    let path = target_dir.join(".twapp-thread.json");

    if !path.exists() {
        eprintln!("No thread followed in {}", target_dir.display());
        return 1;
    }

    if let Err(e) = std::fs::remove_file(&path) {
        eprintln!("Error removing thread file: {}", e);
        return 1;
    }

    println!("Unfollowed thread.");
    0
}

pub fn cmd_thread_status(dir: Option<&str>) -> i32 {
    let target_dir = resolve_dir(dir);

    let meta = match read_thread_meta(&target_dir) {
        Some(m) => m,
        None => {
            println!("No thread followed.");
            return 0;
        }
    };

    println!("Channel: #{}", meta.channel_name);
    println!("Thread: {}", meta.thread_ts);
    println!(
        "URL: {}/archives/{}/p{}",
        meta.workspace_url,
        meta.channel_id,
        meta.thread_ts.replace('.', "")
    );
    println!("Followed: {}", meta.followed_at);
    0
}

pub fn cmd_thread_messages(limit: u32, dir: Option<&str>) -> i32 {
    let target_dir = resolve_dir(dir);

    let meta = match read_thread_meta(&target_dir) {
        Some(m) => m,
        None => {
            eprintln!("No thread followed. Run: twapp thread follow <url>");
            return 1;
        }
    };

    let messages = match fetch_thread_messages(&meta.channel_id, &meta.thread_ts, limit) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    if messages.is_empty() {
        println!("No messages in thread.");
        return 0;
    }

    // Resolve user names
    let user_ids: Vec<String> = messages
        .iter()
        .map(|m| m.user.clone())
        .filter(|id| !id.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let names = resolve_user_names(&user_ids);

    for msg in &messages {
        let author = names.get(&msg.user).unwrap_or(&msg.user);
        let time = format_ts(&msg.ts);
        println!("[{}] {}: {}", time, author, msg.text);

        if let Some(ref files) = msg.files {
            for f in files {
                println!("  📎 {} ({})", f.name, f.mimetype);
            }
        }
        if let Some(ref reactions) = msg.reactions {
            let r: Vec<String> = reactions.iter().map(|r| format!(":{}:{}", r.name, r.count)).collect();
            if !r.is_empty() {
                println!("  {}", r.join(" "));
            }
        }
    }

    0
}

pub fn cmd_thread_reply(text: &str, dir: Option<&str>) -> i32 {
    let target_dir = resolve_dir(dir);

    let meta = match read_thread_meta(&target_dir) {
        Some(m) => m,
        None => {
            eprintln!("No thread followed. Run: twapp thread follow <url>");
            return 1;
        }
    };

    if let Err(e) = send_thread_reply(&meta.channel_id, &meta.thread_ts, text) {
        eprintln!("Error: {}", e);
        return 1;
    }

    println!("Reply sent to #{}", meta.channel_name);
    0
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    if let Some(d) = dir {
        PathBuf::from(d)
    } else {
        std::env::current_dir().unwrap_or_default()
    }
}

fn format_ts(ts: &str) -> String {
    if let Ok(epoch) = ts.parse::<f64>() {
        let secs = epoch as i64;
        let nanos = ((epoch - secs as f64) * 1_000_000_000.0) as u32;
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, nanos) {
            let local: chrono::DateTime<chrono::Local> = dt.into();
            return local.format("%Y-%m-%d %H:%M").to_string();
        }
    }
    ts.to_string()
}
