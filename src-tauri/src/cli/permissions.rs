use std::path::PathBuf;

fn default_permissions_file() -> PathBuf {
    dirs::home_dir()
        .expect("No home directory")
        .join(".config/twapp/default-permissions.json")
}

/// Load default permission patterns from ~/.config/twapp/default-permissions.json.
/// Returns an empty vec if the file doesn't exist or can't be parsed.
pub fn load_default_permissions() -> Vec<String> {
    let path = default_permissions_file();
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Vec<String>>(&content).ok())
        .unwrap_or_default()
}

pub fn save_default_permissions(perms: &[String]) -> Result<(), String> {
    let path = default_permissions_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json =
        serde_json::to_string_pretty(perms).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write: {}", e))
}

pub fn cmd_list() -> i32 {
    let perms = load_default_permissions();
    if perms.is_empty() {
        println!("No default permissions configured.");
        println!("\nAdd with: twapp permissions add 'Bash(gh:*)'");
        return 0;
    }

    println!("Default permissions ({} patterns):\n", perms.len());
    let mut sorted = perms.clone();
    sorted.sort();
    for perm in &sorted {
        println!("  {}", perm);
    }
    println!("\nConfig file: {}", default_permissions_file().display());
    0
}

pub fn cmd_add(pattern: &str) -> i32 {
    let mut perms = load_default_permissions();

    if perms.contains(&pattern.to_string()) {
        println!("Already exists: {}", pattern);
        return 0;
    }

    perms.push(pattern.to_string());
    if let Err(e) = save_default_permissions(&perms) {
        eprintln!("Error: {}", e);
        return 1;
    }
    println!("Added: {}", pattern);
    println!("Total: {} patterns", perms.len());
    0
}

pub fn cmd_remove(pattern: &str) -> i32 {
    let perms = load_default_permissions();

    let matches: Vec<&String> = perms.iter().filter(|p| p.contains(pattern)).collect();

    if matches.is_empty() {
        eprintln!("No pattern matching '{}'", pattern);
        return 1;
    }

    if matches.len() > 1 && !perms.contains(&pattern.to_string()) {
        eprintln!("Ambiguous pattern '{}' matches:", pattern);
        for m in matches.iter().take(10) {
            eprintln!("  {}", m);
        }
        if matches.len() > 10 {
            eprintln!("  ... and {} more", matches.len() - 10);
        }
        eprintln!("\nUse the exact pattern to remove.");
        return 1;
    }

    let to_remove = if perms.contains(&pattern.to_string()) {
        pattern.to_string()
    } else {
        matches[0].clone()
    };

    let remaining: Vec<String> = perms.into_iter().filter(|p| p != &to_remove).collect();
    if let Err(e) = save_default_permissions(&remaining) {
        eprintln!("Error: {}", e);
        return 1;
    }
    println!("Removed: {}", to_remove);
    println!("Remaining: {} patterns", remaining.len());
    0
}

pub fn cmd_sync(dir: Option<&str>) -> i32 {
    let work_dir = if let Some(d) = dir {
        let p = PathBuf::from(d);
        p.canonicalize().unwrap_or(p)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    let session_file = work_dir.join(".twapp-session.json");
    if !session_file.exists() {
        eprintln!("No .twapp-session.json in {}", work_dir.display());
        eprintln!("Run from a twapp session directory or use --dir.");
        return 1;
    }

    let default_perms = load_default_permissions();
    if default_perms.is_empty() {
        println!("No default permissions configured.");
        println!("Add some with: twapp permissions add 'Bash(gh:*)'");
        return 0;
    }

    let claude_json = dirs::home_dir()
        .expect("No home directory")
        .join(".claude.json");
    let dir_key = work_dir.to_string_lossy().to_string();

    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        let mut data: serde_json::Value = if claude_json.exists() {
            let content = std::fs::read_to_string(&claude_json)?;
            serde_json::from_str(&content)?
        } else {
            serde_json::json!({})
        };

        let projects = data
            .as_object_mut()
            .unwrap()
            .entry("projects")
            .or_insert_with(|| serde_json::json!({}));
        let project = projects
            .as_object_mut()
            .unwrap()
            .entry(&dir_key)
            .or_insert_with(|| serde_json::json!({}));

        let mut existing: std::collections::BTreeSet<String> = project
            .get("allowedTools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut added = Vec::new();
        for perm in &default_perms {
            if existing.insert(perm.clone()) {
                added.push(perm.clone());
            }
        }

        if added.is_empty() {
            println!(
                "All {} default permissions already present.",
                default_perms.len()
            );
            return Ok(());
        }

        let sorted: Vec<serde_json::Value> = existing
            .into_iter()
            .map(serde_json::Value::String)
            .collect();
        project
            .as_object_mut()
            .unwrap()
            .insert("allowedTools".to_string(), serde_json::Value::Array(sorted));

        std::fs::write(&claude_json, serde_json::to_string_pretty(&data)?)?;

        println!(
            "Added {} permissions to {}:",
            added.len(),
            work_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        added.sort();
        for perm in added.iter().take(10) {
            println!("  + {}", perm);
        }
        if added.len() > 10 {
            println!("  ... and {} more", added.len() - 10);
        }
        Ok(())
    })();

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error updating permissions: {}", e);
            1
        }
    }
}

/// GUI-friendly: adds a permission and returns the updated list
pub fn add_permission(pattern: &str) -> Result<Vec<String>, String> {
    let mut perms = load_default_permissions();
    if perms.contains(&pattern.to_string()) {
        return Ok(perms);
    }
    perms.push(pattern.to_string());
    save_default_permissions(&perms)?;
    Ok(perms)
}

/// GUI-friendly: removes a permission and returns the updated list
pub fn remove_permission(pattern: &str) -> Result<Vec<String>, String> {
    let perms = load_default_permissions();
    let remaining: Vec<String> = perms.into_iter().filter(|p| p != pattern).collect();
    save_default_permissions(&remaining)?;
    Ok(remaining)
}
