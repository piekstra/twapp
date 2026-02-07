use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::session::SessionData;

#[derive(Debug, Serialize, Deserialize)]
struct QuickPrompt {
    id: String,
    title: String,
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptSection {
    id: String,
    title: String,
    prompts: Vec<QuickPrompt>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptStore {
    sections: Vec<PromptSection>,
}

fn resolve_global_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".config/twapp/quick-prompts.json")
}

fn resolve_project_path(work_dir: &Path) -> PathBuf {
    let session_file = work_dir.join(".twapp-session.json");
    let name = if session_file.exists() {
        std::fs::read_to_string(&session_file)
            .ok()
            .and_then(|c| serde_json::from_str::<SessionData>(&c).ok())
            .map(|s| s.name)
            .unwrap_or_else(|| "twapp".to_string())
    } else {
        "twapp".to_string()
    };
    let safe_name = name.replace(' ', "-").replace('/', "-");
    if safe_name == "twapp" {
        work_dir.join(".twapp-prompts.json")
    } else {
        work_dir.join(format!(".twapp-prompts-{}.json", safe_name))
    }
}

fn load_store(path: &Path) -> PromptStore {
    if !path.exists() {
        return PromptStore {
            sections: Vec::new(),
        };
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(PromptStore {
            sections: Vec::new(),
        })
}

fn save_store(path: &Path, store: &PromptStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }
    let json =
        serde_json::to_string_pretty(store).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write: {}", e))
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    if let Some(d) = dir {
        let p = PathBuf::from(d);
        p.canonicalize().unwrap_or(p)
    } else {
        std::env::current_dir().unwrap_or_default()
    }
}

fn resolve_path(global: bool, dir: Option<&str>) -> PathBuf {
    if global {
        resolve_global_path()
    } else {
        resolve_project_path(&resolve_dir(dir))
    }
}

fn scope_label(global: bool) -> &'static str {
    if global {
        "global"
    } else {
        "project"
    }
}

pub fn cmd_prompt_list(global: bool, dir: Option<&str>) -> i32 {
    let path = resolve_path(global, dir);
    let store = load_store(&path);

    if store.sections.is_empty() {
        println!("No {} prompts.", scope_label(global));
        return 0;
    }

    for section in &store.sections {
        println!("  [{}]", section.title);
        for prompt in &section.prompts {
            let preview = if prompt.text.len() > 60 {
                format!("{}...", &prompt.text[..60])
            } else {
                prompt.text.clone()
            };
            println!(
                "    [{}] {:<20} \"{}\"",
                &prompt.id[..8],
                prompt.title,
                preview
            );
        }
    }
    0
}

pub fn cmd_prompt_add(
    title: &str,
    text: &str,
    section_name: Option<&str>,
    global: bool,
    dir: Option<&str>,
) -> i32 {
    let path = resolve_path(global, dir);
    let mut store = load_store(&path);

    let target_section_name = section_name.unwrap_or("General");

    // Find existing section (case-insensitive)
    let section_idx = store
        .sections
        .iter()
        .position(|s| s.title.eq_ignore_ascii_case(target_section_name));

    let section_idx = match section_idx {
        Some(idx) => idx,
        None => {
            // Create new section
            store.sections.push(PromptSection {
                id: uuid::Uuid::new_v4().to_string(),
                title: target_section_name.to_string(),
                prompts: Vec::new(),
            });
            store.sections.len() - 1
        }
    };

    let prompt = QuickPrompt {
        id: uuid::Uuid::new_v4().to_string(),
        title: title.to_string(),
        text: text.to_string(),
    };

    let actual_section_title = store.sections[section_idx].title.clone();
    store.sections[section_idx].prompts.push(prompt);

    if let Err(e) = save_store(&path, &store) {
        eprintln!("Error: {}", e);
        return 1;
    }

    println!(
        "Added prompt \"{}\" to section \"{}\" ({})",
        title,
        actual_section_title,
        scope_label(global)
    );
    0
}

pub fn cmd_prompt_remove(id_prefix: &str, global: bool, dir: Option<&str>) -> i32 {
    let path = resolve_path(global, dir);
    let mut store = load_store(&path);

    if store.sections.is_empty() {
        eprintln!("No {} prompts.", scope_label(global));
        return 1;
    }

    let prefix = id_prefix.to_lowercase();

    // Find all matching prompts across sections
    let mut matches: Vec<(usize, usize, String, String)> = Vec::new(); // (section_idx, prompt_idx, title, section_title)
    for (si, section) in store.sections.iter().enumerate() {
        for (pi, prompt) in section.prompts.iter().enumerate() {
            if prompt.id.to_lowercase().starts_with(&prefix) {
                matches.push((si, pi, prompt.title.clone(), section.title.clone()));
            }
        }
    }

    if matches.is_empty() {
        eprintln!("No prompt found matching '{}'", id_prefix);
        return 1;
    }
    if matches.len() > 1 {
        eprintln!(
            "Ambiguous ID '{}' matches {} prompts. Use a longer prefix.",
            id_prefix,
            matches.len()
        );
        return 1;
    }

    let (section_idx, _prompt_idx, prompt_title, section_title) = &matches[0];
    let section_idx = *section_idx;
    let prompt_title = prompt_title.clone();
    let section_title = section_title.clone();

    // Remove the prompt
    store.sections[section_idx]
        .prompts
        .retain(|p| !p.id.to_lowercase().starts_with(&prefix));

    if let Err(e) = save_store(&path, &store) {
        eprintln!("Error: {}", e);
        return 1;
    }

    println!(
        "Removed prompt \"{}\" from section \"{}\" ({})",
        prompt_title,
        section_title,
        scope_label(global)
    );
    0
}
