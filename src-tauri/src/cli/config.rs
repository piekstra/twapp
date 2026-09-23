use serde::Deserialize;
use std::path::PathBuf;

use super::session::AgentProvider;

#[derive(Debug, Deserialize)]
struct ConfigYaml {
    defaults: Option<ConfigDefaults>,
}

#[derive(Debug, Deserialize)]
struct ConfigDefaults {
    work_directory: Option<String>,
    jira_project: Option<String>,
    jira_base_url: Option<String>,
    github_repo: Option<String>,
    agent_provider: Option<AgentProvider>,
    agent_providers: Option<Vec<AgentProvider>>,
}

#[derive(Debug)]
pub struct GlobalConfig {
    pub work_directory: PathBuf,
    pub jira_project: Option<String>,
    pub jira_base_url: Option<String>,
    pub github_repo: Option<String>,
    pub agent_provider: AgentProvider,
    pub agent_providers: Vec<AgentProvider>,
}

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("No home directory")
}

fn config_dir() -> PathBuf {
    home_dir().join(".config/twapp")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.yaml")
}

/// Expand leading ~ to home directory
fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else if path == "~" {
        home_dir()
    } else {
        let p = PathBuf::from(path);
        if p.is_relative() {
            std::env::current_dir().unwrap_or_default().join(p)
        } else {
            p
        }
    }
}

pub fn get_theme_preference() -> String {
    let path = config_file();
    if !path.exists() {
        return "system".to_string();
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(theme) = yaml.get("theme").and_then(|v| v.as_str()) {
                return theme.to_string();
            }
        }
    }
    "system".to_string()
}

pub fn set_theme_preference(mode: &str) -> Result<(), String> {
    let path = config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut yaml = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        map.insert(
            serde_yaml::Value::String("theme".to_string()),
            serde_yaml::Value::String(mode.to_string()),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn get_font_family_preference() -> String {
    let path = config_file();
    if !path.exists() {
        return r#""SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace"#.to_string();
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(font) = yaml.get("font_family").and_then(|v| v.as_str()) {
                return font.to_string();
            }
        }
    }
    r#""SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace"#.to_string()
}

pub fn get_session_color_preference() -> String {
    let path = config_file();
    if !path.exists() {
        return "random".to_string();
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(color) = yaml.get("session_color").and_then(|v| v.as_str()) {
                return color.to_string();
            }
        }
    }
    "random".to_string()
}

pub fn get_agent_provider_preference() -> AgentProvider {
    let path = config_file();
    if !path.exists() {
        return AgentProvider::Claude;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(provider) = yaml.get("agent_provider").and_then(|v| v.as_str()) {
                return AgentProvider::parse(provider).unwrap_or(AgentProvider::Claude);
            }
        }
    }
    AgentProvider::Claude
}

pub fn get_configured_agent_providers() -> Vec<AgentProvider> {
    GlobalConfig::load()
        .map(|config| config.agent_providers)
        .unwrap_or_else(|_| vec![AgentProvider::Claude])
}

/// Find `provider`'s command, re-reading PATH from an interactive shell before
/// concluding it is absent.
///
/// twapp does not run the harness itself: it writes the command into a
/// terminal whose shell resolves it. That shell reads startup files the PATH
/// twapp starts with may not reflect, so a first miss means "ask the shell
/// again", not "not installed".
pub fn locate_agent_provider_binary(provider: AgentProvider) -> Option<PathBuf> {
    if let Some(found) = find_agent_provider_binary(provider) {
        return Some(found);
    }
    crate::gui::shell_env::refresh_path().ok()?;
    find_agent_provider_binary(provider)
}

pub fn find_agent_provider_binary(provider: AgentProvider) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for binary in provider.binaries() {
            let candidate = directory.join(binary);
            if candidate
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn normalize_agent_providers(
    configured: Option<Vec<AgentProvider>>,
    fallback: AgentProvider,
) -> Vec<AgentProvider> {
    let mut providers = Vec::new();
    for provider in configured.unwrap_or_else(|| vec![fallback]) {
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    if providers.is_empty() {
        providers.push(fallback);
    }
    providers
}

pub fn set_agent_provider_preference(provider: AgentProvider) -> Result<(), String> {
    let path = config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut yaml = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        map.insert(
            serde_yaml::Value::String("agent_provider".to_string()),
            serde_yaml::Value::String(provider.to_string()),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn set_session_color_preference(mode: &str) -> Result<(), String> {
    let path = config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut yaml = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        map.insert(
            serde_yaml::Value::String("session_color".to_string()),
            serde_yaml::Value::String(mode.to_string()),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn save_global_config(
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
    agent_provider: Option<String>,
    agent_providers: Option<Vec<String>>,
) -> Result<(), String> {
    let path = config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut yaml = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    if let serde_yaml::Value::Mapping(ref mut map) = yaml {
        let defaults = map
            .entry(serde_yaml::Value::String("defaults".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        if let serde_yaml::Value::Mapping(ref mut dmap) = defaults {
            if let Some(wd) = work_directory {
                dmap.insert(
                    serde_yaml::Value::String("work_directory".to_string()),
                    serde_yaml::Value::String(wd),
                );
            }
            if let Some(jp) = jira_project {
                dmap.insert(
                    serde_yaml::Value::String("jira_project".to_string()),
                    if jp.is_empty() { serde_yaml::Value::Null } else { serde_yaml::Value::String(jp) },
                );
            }
            if let Some(gr) = github_repo {
                dmap.insert(
                    serde_yaml::Value::String("github_repo".to_string()),
                    if gr.is_empty() { serde_yaml::Value::Null } else { serde_yaml::Value::String(gr) },
                );
            }
            if let Some(provider) = agent_provider {
                dmap.insert(
                    serde_yaml::Value::String("agent_provider".to_string()),
                    serde_yaml::Value::String(provider),
                );
            }
            if let Some(providers) = agent_providers {
                let parsed = providers
                    .iter()
                    .map(|provider| {
                        AgentProvider::parse(provider)
                            .ok_or_else(|| format!("Unknown agent harness: {}", provider))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if parsed.is_empty() {
                    return Err("Configure at least one agent harness".to_string());
                }
                dmap.insert(
                    serde_yaml::Value::String("agent_providers".to_string()),
                    serde_yaml::to_value(parsed).map_err(|e| e.to_string())?,
                );
            }
        }
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

impl GlobalConfig {
    pub fn load() -> Result<Self, String> {
        let path = config_file();

        if !path.exists() {
            return Ok(Self {
                work_directory: home_dir().join("Dev"),
                jira_project: None,
                jira_base_url: None,
                github_repo: None,
                agent_provider: AgentProvider::Claude,
                agent_providers: vec![AgentProvider::Claude],
            });
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let yaml: ConfigYaml = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let defaults = yaml.defaults.unwrap_or(ConfigDefaults {
            work_directory: None,
            jira_project: None,
            jira_base_url: None,
            github_repo: None,
            agent_provider: None,
            agent_providers: None,
        });

        let work_directory = defaults
            .work_directory
            .map(|p| expand_path(&p))
            .unwrap_or_else(|| home_dir().join("Dev"));

        let agent_provider = defaults.agent_provider.unwrap_or(AgentProvider::Claude);
        let agent_providers = normalize_agent_providers(defaults.agent_providers, agent_provider);

        Ok(Self {
            work_directory,
            jira_project: defaults.jira_project,
            jira_base_url: defaults
                .jira_base_url
                .map(|url| url.trim().to_string())
                .filter(|url| !url.is_empty()),
            github_repo: defaults.github_repo,
            agent_provider,
            agent_providers,
        })
    }
}

#[cfg(test)]
mod agent_provider_tests {
    use super::*;

    #[test]
    fn configured_harnesses_are_deduplicated_without_reordering() {
        let providers = normalize_agent_providers(
            Some(vec![
                AgentProvider::Codex,
                AgentProvider::Claude,
                AgentProvider::Codex,
            ]),
            AgentProvider::Claude,
        );

        assert_eq!(
            providers,
            vec![AgentProvider::Codex, AgentProvider::Claude]
        );
    }

    #[test]
    fn empty_harness_config_falls_back_to_legacy_provider() {
        assert_eq!(
            normalize_agent_providers(Some(Vec::new()), AgentProvider::Codex),
            vec![AgentProvider::Codex]
        );
    }
}

/// The `summaries:` block of `config.yaml`: `(provider, model)`, each unset
/// when absent or blank.
pub fn get_summaries_settings() -> (Option<String>, Option<String>) {
    let Ok(content) = std::fs::read_to_string(config_file()) else {
        return (None, None);
    };
    let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
        return (None, None);
    };
    parse_summaries_settings(&yaml)
}

fn parse_summaries_settings(yaml: &serde_yaml::Value) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        yaml.get("summaries")
            .and_then(|block| block.get(name))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    (field("provider"), field("model"))
}

#[cfg(test)]
mod summaries_settings_tests {
    use super::parse_summaries_settings;

    #[test]
    fn reads_provider_and_model_from_the_summaries_block() {
        let yaml = serde_yaml::from_str("summaries:\n  provider: codex\n  model: fast\n").unwrap();
        assert_eq!(
            parse_summaries_settings(&yaml),
            (Some("codex".to_string()), Some("fast".to_string()))
        );
    }

    #[test]
    fn a_missing_or_blank_block_leaves_both_unset() {
        let yaml = serde_yaml::from_str("theme: dark\n").unwrap();
        assert_eq!(parse_summaries_settings(&yaml), (None, None));
        let yaml = serde_yaml::from_str("summaries:\n  provider: \"  \"\n").unwrap();
        assert_eq!(parse_summaries_settings(&yaml), (None, None));
    }
}
