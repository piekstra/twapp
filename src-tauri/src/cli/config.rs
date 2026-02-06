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
