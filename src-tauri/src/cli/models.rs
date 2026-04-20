use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub name: String,
    pub tier: String,
    pub description: String,
}

const BUNDLED_CLAUDE_DEFAULT: &str =
    include_str!("../../../data/models.claude.default.json");

pub fn cache_path(provider: &str) -> PathBuf {
    let base = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("twapp");
    base.join(format!("models.{}.json", provider))
}

pub fn bundled_default(provider: &str) -> Result<Vec<ModelEntry>, String> {
    match provider {
        "claude" => serde_json::from_str(BUNDLED_CLAUDE_DEFAULT)
            .map_err(|e| format!("parse bundled claude default: {}", e)),
        "codex" => Ok(Vec::new()),
        other => Err(format!("unknown provider: {}", other)),
    }
}

pub fn load_models_from(cache: &Path, provider: &str) -> Result<(Vec<ModelEntry>, &'static str), String> {
    if cache.exists() {
        let s = std::fs::read_to_string(cache)
            .map_err(|e| format!("read cache {}: {}", cache.display(), e))?;
        let entries: Vec<ModelEntry> = serde_json::from_str(&s)
            .map_err(|e| format!("parse cache {}: {}", cache.display(), e))?;
        Ok((entries, "cache"))
    } else {
        bundled_default(provider).map(|e| (e, "bundled"))
    }
}

pub fn parse_anthropic_response(body: &str) -> Result<Vec<ModelEntry>, String> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<AModel>,
    }
    #[derive(Deserialize)]
    struct AModel {
        id: String,
        #[serde(default)]
        display_name: String,
    }
    let resp: Resp =
        serde_json::from_str(body).map_err(|e| format!("parse anthropic response: {}", e))?;
    Ok(resp
        .data
        .into_iter()
        .map(|m| {
            let tier = tier_for_name(&m.id).to_string();
            let description = if m.display_name.is_empty() {
                format!("{}-tier Claude model.", tier)
            } else {
                m.display_name
            };
            ModelEntry {
                name: m.id,
                tier,
                description,
            }
        })
        .collect())
}

pub fn tier_for_name(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("opus") {
        "opus"
    } else if n.contains("sonnet") {
        "sonnet"
    } else if n.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

pub fn write_cache_atomic(path: &Path, entries: &[ModelEntry]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {}", parent.display(), e))?;
    }
    let tmp = path.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(entries).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&tmp, s).map_err(|e| format!("write temp {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename to {}: {}", path.display(), e))?;
    Ok(())
}

pub fn cmd_list(provider: String, format: Option<String>) -> i32 {
    let cache = cache_path(&provider);
    let (entries, source) = match load_models_from(&cache, &provider) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    match format.as_deref() {
        Some("json") => match serde_json::to_string_pretty(&entries) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        },
        Some(other) => {
            eprintln!("Error: unknown --format {} (want: json)", other);
            2
        }
        None => {
            if entries.is_empty() {
                println!("no models cached for {} (source: {})", provider, source);
                return 0;
            }
            let name_w = entries
                .iter()
                .map(|e| e.name.len())
                .max()
                .unwrap_or(4)
                .max(4);
            let tier_w = entries
                .iter()
                .map(|e| e.tier.len())
                .max()
                .unwrap_or(4)
                .max(4);
            println!(
                "{:<nw$}  {:<tw$}  DESCRIPTION",
                "NAME",
                "TIER",
                nw = name_w,
                tw = tier_w
            );
            for e in &entries {
                println!(
                    "{:<nw$}  {:<tw$}  {}",
                    e.name,
                    e.tier,
                    e.description,
                    nw = name_w,
                    tw = tier_w
                );
            }
            eprintln!("(source: {source})");
            0
        }
    }
}

pub fn cmd_refresh(provider: String) -> i32 {
    match provider.as_str() {
        "claude" => refresh_claude(),
        "codex" => {
            let path = cache_path("codex");
            eprintln!(
                "Error: refresh not supported for codex yet; edit the cache by hand at {}",
                path.display()
            );
            1
        }
        other => {
            eprintln!("Error: unknown provider: {}", other);
            1
        }
    }
}

fn refresh_claude() -> i32 {
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "Error: ANTHROPIC_API_KEY is not set. Export ANTHROPIC_API_KEY and retry."
            );
            return 1;
        }
    };

    let output = match std::process::Command::new("curl")
        .arg("-sS")
        .arg("-f")
        .arg("-H")
        .arg(format!("x-api-key: {}", api_key))
        .arg("-H")
        .arg("anthropic-version: 2023-06-01")
        .arg("https://api.anthropic.com/v1/models")
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error: failed to invoke curl: {}", e);
            return 1;
        }
    };

    if !output.status.success() {
        eprintln!(
            "Error: curl exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return 1;
    }

    let body = String::from_utf8_lossy(&output.stdout).into_owned();
    let entries = match parse_anthropic_response(&body) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let path = cache_path("claude");
    if let Err(e) = write_cache_atomic(&path, &entries) {
        eprintln!("Error: {}", e);
        return 1;
    }

    println!(
        "refreshed claude models: {} entries cached at {}",
        entries.len(),
        path.display()
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_default_parses() {
        let entries = bundled_default("claude").expect("bundled default parses");
        assert!(!entries.is_empty(), "bundled default must not be empty");
        for e in &entries {
            assert!(!e.name.is_empty());
            assert!(matches!(e.tier.as_str(), "opus" | "sonnet" | "haiku" | "other"));
            assert!(!e.description.is_empty());
        }
    }

    #[test]
    fn models_list_loads_bundled_default_when_cache_missing() {
        let tmp = tempdir();
        let cache = tmp.join("models.claude.json");
        assert!(!cache.exists());
        let (entries, source) =
            load_models_from(&cache, "claude").expect("load falls back to bundled");
        assert_eq!(source, "bundled");
        assert!(!entries.is_empty());
    }

    #[test]
    fn models_list_prefers_cache_over_bundled_when_both_exist() {
        let tmp = tempdir();
        let cache = tmp.join("models.claude.json");
        let canned = vec![ModelEntry {
            name: "cache-only-model".to_string(),
            tier: "haiku".to_string(),
            description: "from the cache".to_string(),
        }];
        write_cache_atomic(&cache, &canned).expect("write cache");
        let (entries, source) =
            load_models_from(&cache, "claude").expect("load from cache");
        assert_eq!(source, "cache");
        assert_eq!(entries, canned);
    }

    #[test]
    fn models_refresh_parses_anthropic_response() {
        let body = r#"{
          "data": [
            {"id": "claude-opus-4-7", "display_name": "Claude Opus 4.7"},
            {"id": "claude-sonnet-4-6", "display_name": "Claude Sonnet 4.6"},
            {"id": "claude-haiku-4-5-20251001", "display_name": "Claude Haiku 4.5"}
          ],
          "has_more": false
        }"#;
        let entries = parse_anthropic_response(body).expect("parse");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "claude-opus-4-7");
        assert_eq!(entries[0].tier, "opus");
        assert_eq!(entries[0].description, "Claude Opus 4.7");
        assert_eq!(entries[1].tier, "sonnet");
        assert_eq!(entries[2].tier, "haiku");
    }

    #[test]
    fn models_refresh_errors_without_api_key() {
        let prior = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let code = refresh_claude();
        if let Some(v) = prior {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
        assert_eq!(code, 1);
    }

    #[test]
    fn tier_for_name_matches_family() {
        assert_eq!(tier_for_name("claude-opus-4-7"), "opus");
        assert_eq!(tier_for_name("claude-sonnet-4-6"), "sonnet");
        assert_eq!(tier_for_name("claude-haiku-4-5"), "haiku");
        assert_eq!(tier_for_name("something-else"), "other");
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "twapp-models-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("create tempdir");
        base
    }
}
