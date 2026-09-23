use std::path::{Path, PathBuf};

const CODESIGN_IDENTITY: &str = "twapp-codesign";

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("No home directory")
}

pub fn gui_app_path() -> PathBuf {
    home_dir().join(".config/twapp/twapp.app")
}

fn instances_dir() -> PathBuf {
    home_dir().join(".config/twapp/instances")
}

/// Check that the GUI .app bundle is installed
pub fn check_gui_installed() -> Result<(), String> {
    let app = gui_app_path();
    if !app.exists() {
        Err(format!(
            "Error: twapp not found at {}\nRun 'twapp install-gui <path-to-binary>' first.",
            app.display()
        ))
    } else {
        Ok(())
    }
}

/// Remove any entries in the .app bundle root that aren't `Contents/`.
/// Stray files or symlinks cause codesign "unsealed contents" errors.
pub fn clean_bundle_root(app_path: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(app_path).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() != "Contents" {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
            } else {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Re-sign a .app bundle with the twapp-codesign certificate.
/// Falls back to ad-hoc signing if the certificate is not found.
pub fn resign_app_bundle(app_path: &Path) -> Result<(), String> {
    let result = std::process::Command::new("codesign")
        .args([
            "--force",
            "--deep",
            "-s",
            CODESIGN_IDENTITY,
            &app_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("codesign failed: {}", e))?;

    if result.status.success() {
        println!(
            "Signed {} (identity: {})",
            app_path.file_name().unwrap().to_string_lossy(),
            CODESIGN_IDENTITY
        );
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        if stderr.to_lowercase().contains("no identity found") {
            // Fallback to ad-hoc signing
            println!(
                "Warning: '{}' certificate not found, using ad-hoc signing.",
                CODESIGN_IDENTITY
            );
            println!("Run 'twapp setup-cert' to create the certificate and stop permission prompts.");
            let fallback = std::process::Command::new("codesign")
                .args([
                    "--force",
                    "--deep",
                    "-s",
                    "-",
                    &app_path.to_string_lossy(),
                ])
                .output()
                .map_err(|e| format!("codesign fallback failed: {}", e))?;
            if fallback.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "codesign failed: {}",
                    String::from_utf8_lossy(&fallback.stderr)
                ))
            }
        } else {
            Err(format!("codesign failed: {}", stderr))
        }
    }
}

/// Remove the per-session app bundles older versions cloned for every session
/// window. A bundle whose process is still running is left alone.
pub fn remove_legacy_instances() -> usize {
    let dir = instances_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_bundle = path.extension().and_then(|e| e.to_str()) == Some("app");
        if is_bundle {
            let needle = format!("{}/Contents/MacOS/", path.to_string_lossy());
            let running = std::process::Command::new("pgrep")
                .args(["-f", &needle])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(true);
            if running {
                continue;
            }
            if std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        } else if path.to_string_lossy().ends_with(".args.json") {
            let bundle = path.to_string_lossy().trim_end_matches(".args.json").to_string() + ".app";
            if !std::path::Path::new(&bundle).exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    let _ = std::fs::remove_dir(&dir);
    removed
}
