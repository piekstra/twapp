use super::types::*;
use std::process::Command;
use std::path::{Path, PathBuf};

fn resolve_preview_path(path: &str, cwd: Option<&str>) -> Result<PathBuf, String> {
    let workspace_root = std::fs::canonicalize(cwd.unwrap_or("."))
        .map_err(|e| format!("Failed to resolve workspace root: {}", e))?;
    let requested = Path::new(path);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace_root.join(requested)
    };

    if !candidate.exists() {
        return Err(format!("File not found: {}", candidate.display()));
    }

    let resolved = std::fs::canonicalize(&candidate).map_err(|e| e.to_string())?;
    if !resolved.starts_with(&workspace_root) {
        return Err("File preview is limited to the current workspace".to_string());
    }

    Ok(resolved)
}

#[tauri::command]
pub fn read_file(path: String, config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let resolved = resolve_preview_path(&path, config.cwd.as_deref())?;
    let metadata = std::fs::metadata(&resolved).map_err(|e| e.to_string())?;
    if metadata.len() > 2 * 1024 * 1024 {
        return Err("File too large (max 2MB)".to_string());
    }
    std::fs::read_to_string(&resolved).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_file_base64(path: String, config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let resolved = resolve_preview_path(&path, config.cwd.as_deref())?;
    let metadata = std::fs::metadata(&resolved).map_err(|e| e.to_string())?;
    if metadata.len() > 5 * 1024 * 1024 {
        return Err("File too large (max 5MB)".to_string());
    }
    let bytes = std::fs::read(&resolved).map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

#[tauri::command]
pub fn dev_reload(config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let cwd = config.cwd.clone().unwrap_or_else(|| ".".to_string());
    let pid = std::process::id();

    // Log file so the user can see build progress/errors
    let log_path = std::path::Path::new(&cwd).join(".twapp-rebuild.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| format!("Failed to create log file: {}", e))?;
    let log_err = log_file
        .try_clone()
        .map_err(|e| format!("Failed to clone log file: {}", e))?;

    // Spawn via login shell so PATH includes cargo, npm, etc.
    let cmd = format!("twapp dev-reload --pid {} --cwd '{}'", pid, cwd.replace('\'', "'\\''"));
    Command::new("/bin/zsh")
        .args(["-lc", &cmd])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_err))
        .spawn()
        .map_err(|e| format!("Failed to spawn dev-reload: {}", e))?;

    Ok(log_path.to_string_lossy().to_string())
}

/// Close the current instance and relaunch.
/// In session mode: `twapp resume` in the session's cwd.
/// In launcher mode (no session): just `twapp` to reopen the launcher.
#[tauri::command]
pub fn reload_app(config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let is_launcher = config.session_id.is_none() && config.command.is_none();

    let cmd = if is_launcher {
        "twapp".to_string()
    } else {
        let cwd = config.cwd.clone().unwrap_or_else(|| ".".to_string());
        format!("cd '{}' && twapp resume", cwd.replace('\'', "'\\''"))
    };

    // Use login shell so PATH includes twapp
    Command::new("/bin/zsh")
        .args(["-lc", &cmd])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to relaunch twapp: {}", e))?;

    // Exit current instance after brief delay for spawn
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    });

    Ok(())
}

#[tauri::command]
pub fn read_rebuild_log(config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let log_path = std::path::Path::new(cwd).join(".twapp-rebuild.log");
    if log_path.exists() {
        std::fs::read_to_string(&log_path).map_err(|e| e.to_string())
    } else {
        Ok(String::new())
    }
}

#[tauri::command]
pub async fn install_update(download_url: String) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join(format!("twapp-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let tarball = tmp_dir.join("twapp-macos-aarch64.tar.gz");
    let extract_dir = tmp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    // Download with curl
    let output = tokio::process::Command::new("curl")
        .args([
            "-fSL",
            "--connect-timeout",
            "30",
            "--max-time",
            "120",
            "-o",
            &tarball.to_string_lossy(),
            &download_url,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run curl: {}", e))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Download failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Extract
    let output = tokio::process::Command::new("tar")
        .args([
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &extract_dir.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to extract: {}", e))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Find the .app bundle
    let app_source = extract_dir.join("twapp.app");
    if !app_source.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("twapp.app not found in release archive".to_string());
    }

    // Replace master bundle
    let target = crate::cli::app_bundle::gui_app_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("Failed to remove old bundle: {}", e))?;
    }

    let cp_output = tokio::process::Command::new("cp")
        .args([
            "-R",
            &app_source.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to copy bundle: {}", e))?;

    if !cp_output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Copy failed: {}",
            String::from_utf8_lossy(&cp_output.stderr)
        ));
    }

    // Clean stray files from bundle root before signing
    crate::cli::app_bundle::clean_bundle_root(&target)?;

    // Re-sign
    crate::cli::app_bundle::resign_app_bundle(&target)?;

    // Clean up
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok("Update installed successfully".to_string())
}

#[tauri::command]
pub fn reveal_in_finder(path: String) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to reveal: {}", e))?;
    Ok(())
}
