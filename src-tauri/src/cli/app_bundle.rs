use std::path::{Path, PathBuf};

const CODESIGN_IDENTITY: &str = "twapp-codesign";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

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

/// Create a per-instance .app bundle clone with a custom CFBundleName.
/// Uses APFS clonefile (cp -Rc) so the copy is nearly instant and shares
/// storage with the master bundle (copy-on-write).
pub fn prepare_instance_app(name: &str) -> Result<PathBuf, String> {
    let instances = instances_dir();
    std::fs::create_dir_all(&instances).map_err(|e| e.to_string())?;

    // Sanitize name for filesystem: keep word chars, spaces, hyphens
    let safe: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .collect();
    let safe = safe.trim().replace(' ', "-");
    let safe = if safe.is_empty() {
        "twapp".to_string()
    } else {
        safe[..safe.len().min(64)].to_string()
    };

    let instance_app = instances.join(format!("{}.app", safe));

    // Remove stale instance
    if instance_app.exists() {
        std::fs::remove_dir_all(&instance_app).map_err(|e| e.to_string())?;
    }

    // APFS clone (fast, CoW)
    let output = std::process::Command::new("cp")
        .args([
            "-Rc",
            &gui_app_path().to_string_lossy(),
            &instance_app.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("cp failed: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "cp -Rc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Patch CFBundleName/CFBundleDisplayName but keep the shared
    // CFBundleIdentifier so TCC remembers permission grants across instances.
    let plist_path = instance_app.join("Contents/Info.plist");
    let mut plist_data: plist::Dictionary = plist::from_file(&plist_path)
        .map_err(|e| format!("Failed to read plist: {}", e))?;

    plist_data.insert(
        "CFBundleName".to_string(),
        plist::Value::String(name.to_string()),
    );
    plist_data.insert(
        "CFBundleDisplayName".to_string(),
        plist::Value::String(name.to_string()),
    );

    let mut file = std::fs::File::create(&plist_path).map_err(|e| e.to_string())?;
    plist::to_writer_binary(&mut file, &plist_data)
        .map_err(|e| format!("Failed to write plist: {}", e))?;

    // Re-sign so macOS accepts the modified bundle
    resign_app_bundle(&instance_app)?;

    // Force Launch Services to re-read this bundle's metadata
    let _ = std::process::Command::new(LSREGISTER)
        .args(["-f", &instance_app.to_string_lossy()])
        .output();

    Ok(instance_app)
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

/// Launch a GUI instance via `open -n -a`
pub fn launch_gui(instance_app: &Path, args: &[String]) -> Result<(), String> {
    let mut open_args = vec![
        "-n".to_string(),
        "-a".to_string(),
        instance_app.to_string_lossy().to_string(),
        "--args".to_string(),
    ];
    open_args.extend_from_slice(args);

    std::process::Command::new("open")
        .args(&open_args)
        .spawn()
        .map_err(|e| format!("Failed to launch: {}", e))?;

    Ok(())
}
