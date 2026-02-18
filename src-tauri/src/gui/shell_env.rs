use parking_lot::Mutex;
use std::sync::OnceLock;

/// Cached PATH from user's login shell, refreshable on tool-not-found.
static DISCOVERED_PATH: OnceLock<Mutex<String>> = OnceLock::new();

/// Try to discover PATH by spawning a login shell with the given binary.
fn try_shell(shell: &str) -> Result<String, String> {
    let output = std::process::Command::new(shell)
        .args(["-lc", "echo $PATH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to spawn {}: {}", shell, e))?;

    if !output.status.success() {
        return Err(format!("{} exited with non-zero status", shell));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err(format!("{} returned empty PATH", shell));
    }

    Ok(path)
}

/// Spawn the user's login shell to get their full PATH.
/// Tries $SHELL first, then falls back to /bin/zsh and /bin/bash.
fn discover_path_from_shell() -> Result<String, String> {
    // Try the user's configured shell first
    if let Ok(shell) = std::env::var("SHELL") {
        if let Ok(path) = try_shell(&shell) {
            return Ok(path);
        }
    }

    // Fallback: try common shells
    for shell in &["/bin/zsh", "/bin/bash"] {
        if let Ok(path) = try_shell(shell) {
            return Ok(path);
        }
    }

    Err("All shell attempts failed".to_string())
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
    pub binaries: &'static [&'static str],
    pub name: &'static str,
    pub install_hint: &'static str,
}

pub const TOOL_JTK: ToolInfo = ToolInfo {
    binaries: &["jtk", "jira-ticket-cli"],
    name: "Jira CLI (jtk)",
    install_hint: "Install: brew install open-cli-collective/tap/jtk\nMore info: https://github.com/open-cli-collective/atlassian-cli",
};

pub const TOOL_GH: ToolInfo = ToolInfo {
    binaries: &["gh"],
    name: "GitHub CLI (gh)",
    install_hint: "Install: brew install gh\nMore info: https://cli.github.com",
};

/// Get the currently discovered PATH.
fn get_path() -> String {
    DISCOVERED_PATH
        .get()
        .map(|m| m.lock().clone())
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

fn is_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

fn not_found_message(tool: &ToolInfo) -> String {
    format!("{} not found on your system.\n\n{}", tool.name, tool.install_hint)
}

/// Run a CLI tool asynchronously, trying all known binary names.
/// Explicitly passes the discovered PATH to child processes.
/// If none are found, refreshes PATH and retries once.
pub async fn run_tool(
    tool: &ToolInfo,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let path = get_path();

    // Try each binary name
    for binary in tool.binaries {
        match tokio::process::Command::new(binary)
            .args(args)
            .env("PATH", &path)
            .output()
            .await
        {
            Ok(output) => return Ok(output),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(format!("Failed to run {}: {}", binary, e)),
        }
    }

    // All not found — refresh PATH and retry
    let _ = refresh_path();
    let path = get_path();

    for binary in tool.binaries {
        match tokio::process::Command::new(binary)
            .args(args)
            .env("PATH", &path)
            .output()
            .await
        {
            Ok(output) => return Ok(output),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(format!("Failed to run {}: {}", binary, e)),
        }
    }

    Err(not_found_message(tool))
}

/// Sync version for non-async contexts.
pub fn run_tool_sync(
    tool: &ToolInfo,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let path = get_path();

    for binary in tool.binaries {
        match std::process::Command::new(binary)
            .args(args)
            .env("PATH", &path)
            .output()
        {
            Ok(output) => return Ok(output),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(format!("Failed to run {}: {}", binary, e)),
        }
    }

    let _ = refresh_path();
    let path = get_path();

    for binary in tool.binaries {
        match std::process::Command::new(binary)
            .args(args)
            .env("PATH", &path)
            .output()
        {
            Ok(output) => return Ok(output),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(format!("Failed to run {}: {}", binary, e)),
        }
    }

    Err(not_found_message(tool))
}
