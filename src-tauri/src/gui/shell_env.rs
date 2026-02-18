use parking_lot::Mutex;
use std::sync::OnceLock;

/// Cached PATH from user's login shell, refreshable on tool-not-found.
static DISCOVERED_PATH: OnceLock<Mutex<String>> = OnceLock::new();

/// Spawn a login shell to get the user's full PATH.
fn discover_path_from_shell() -> Result<String, String> {
    let output = std::process::Command::new("/bin/zsh")
        .args(["-lc", "echo $PATH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to spawn login shell: {}", e))?;

    if !output.status.success() {
        return Err("Login shell exited with non-zero status".to_string());
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err("Login shell returned empty PATH".to_string());
    }

    Ok(path)
}

/// Discover the user's PATH and set it process-wide.
/// Call once at GUI startup. Falls back to common paths if discovery fails.
pub fn init_path() {
    let path = match discover_path_from_shell() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: PATH discovery failed ({}), using fallback", e);
            let home = dirs::home_dir().unwrap_or_default();
            let existing = std::env::var("PATH").unwrap_or_default();
            format!(
                "{}/.local/bin:{}/.config/twapp/bin:/opt/homebrew/bin:/usr/local/bin:{}",
                home.display(),
                home.display(),
                existing,
            )
        }
    };

    std::env::set_var("PATH", &path);

    let mutex = DISCOVERED_PATH.get_or_init(|| Mutex::new(String::new()));
    *mutex.lock() = path;
}

/// Re-discover PATH from login shell and update the process environment.
/// Called when a tool-not-found error triggers a retry.
pub fn refresh_path() -> Result<(), String> {
    let path = discover_path_from_shell()?;
    std::env::set_var("PATH", &path);
    if let Some(mutex) = DISCOVERED_PATH.get() {
        *mutex.lock() = path;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

pub struct ToolInfo {
    pub binary: &'static str,
    pub name: &'static str,
    pub install_hint: &'static str,
}

pub const TOOL_JTK: ToolInfo = ToolInfo {
    binary: "jtk",
    name: "Jira CLI (jtk)",
    install_hint: "Install: brew install open-cli-collective/tap/atlassian-cli\nMore info: https://github.com/open-cli-collective/atlassian-cli",
};

pub const TOOL_GH: ToolInfo = ToolInfo {
    binary: "gh",
    name: "GitHub CLI (gh)",
    install_hint: "Install: brew install gh\nMore info: https://cli.github.com",
};

fn is_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

fn not_found_message(tool: &ToolInfo) -> String {
    format!("{} not found on your system.\n\n{}", tool.name, tool.install_hint)
}

/// Run a CLI tool asynchronously with retry-on-not-found.
/// If the binary isn't found, refreshes PATH and retries once.
pub async fn run_tool(
    tool: &ToolInfo,
    args: &[&str],
) -> Result<std::process::Output, String> {
    match tokio::process::Command::new(tool.binary)
        .args(args)
        .output()
        .await
    {
        Ok(output) => Ok(output),
        Err(e) if is_not_found(&e) => {
            let _ = refresh_path();
            tokio::process::Command::new(tool.binary)
                .args(args)
                .output()
                .await
                .map_err(|e2| {
                    if is_not_found(&e2) {
                        not_found_message(tool)
                    } else {
                        format!("Failed to run {}: {}", tool.binary, e2)
                    }
                })
        }
        Err(e) => Err(format!("Failed to run {}: {}", tool.binary, e)),
    }
}

/// Sync version for non-async contexts.
pub fn run_tool_sync(
    tool: &ToolInfo,
    args: &[&str],
) -> Result<std::process::Output, String> {
    match std::process::Command::new(tool.binary)
        .args(args)
        .output()
    {
        Ok(output) => Ok(output),
        Err(e) if is_not_found(&e) => {
            let _ = refresh_path();
            std::process::Command::new(tool.binary)
                .args(args)
                .output()
                .map_err(|e2| {
                    if is_not_found(&e2) {
                        not_found_message(tool)
                    } else {
                        format!("Failed to run {}: {}", tool.binary, e2)
                    }
                })
        }
        Err(e) => Err(format!("Failed to run {}: {}", tool.binary, e)),
    }
}
