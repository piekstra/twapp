use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct ConfigYaml {
    defaults: Option<ConfigDefaults>,
}

#[derive(Debug, Deserialize)]
struct ConfigDefaults {
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
}

#[derive(Debug)]
pub struct GlobalConfig {
    pub work_directory: PathBuf,
    pub jira_project: Option<String>,
    pub github_repo: Option<String>,
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

pub fn get_monitor_position() -> String {
    let path = config_file();
    if !path.exists() {
        return "bottom".to_string();
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(pos) = yaml.get("monitor_position").and_then(|v| v.as_str()) {
                return pos.to_string();
            }
        }
    }
    "bottom".to_string()
}

pub fn set_monitor_position(position: &str) -> Result<(), String> {
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
            serde_yaml::Value::String("monitor_position".to_string()),
            serde_yaml::Value::String(position.to_string()),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn get_monitor_size() -> u32 {
    let path = config_file();
    if !path.exists() {
        return 300;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(size) = yaml.get("monitor_size").and_then(|v| v.as_u64()) {
                return size as u32;
            }
        }
    }
    300
}

pub fn set_monitor_size(size: u32) -> Result<(), String> {
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
            serde_yaml::Value::String("monitor_size".to_string()),
            serde_yaml::Value::Number(serde_yaml::Number::from(size as u64)),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn get_monitor_enabled() -> bool {
    let path = config_file();
    if !path.exists() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(val) = yaml.get("monitor_enabled").and_then(|v| v.as_bool()) {
                return val;
            }
        }
    }
    false
}

pub fn set_monitor_enabled(enabled: bool) -> Result<(), String> {
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
            serde_yaml::Value::String("monitor_enabled".to_string()),
            serde_yaml::Value::Bool(enabled),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn get_monitor_float() -> bool {
    let path = config_file();
    if !path.exists() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(float_val) = yaml.get("monitor_float").and_then(|v| v.as_bool()) {
                return float_val;
            }
        }
    }
    false
}

pub fn set_monitor_float(float: bool) -> Result<(), String> {
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
            serde_yaml::Value::String("monitor_float".to_string()),
            serde_yaml::Value::Bool(float),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn get_slack_enabled() -> bool {
    let path = config_file();
    if !path.exists() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(val) = yaml.get("slack_enabled").and_then(|v| v.as_bool()) {
                return val;
            }
        }
    }
    false
}

pub fn set_slack_enabled(enabled: bool) -> Result<(), String> {
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
            serde_yaml::Value::String("slack_enabled".to_string()),
            serde_yaml::Value::Bool(enabled),
        );
    }

    std::fs::write(&path, serde_yaml::to_string(&yaml).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub fn save_global_config(
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
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
                github_repo: None,
            });
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let yaml: ConfigYaml = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let defaults = yaml.defaults.unwrap_or(ConfigDefaults {
            work_directory: None,
            jira_project: None,
            github_repo: None,
        });

        let work_directory = defaults
            .work_directory
            .map(|p| expand_path(&p))
            .unwrap_or_else(|| home_dir().join("Dev"));

        Ok(Self {
            work_directory,
            jira_project: defaults.jira_project,
            github_repo: defaults.github_repo,
        })
    }
}
