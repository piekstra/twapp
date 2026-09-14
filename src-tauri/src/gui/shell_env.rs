use parking_lot::Mutex;
use std::sync::OnceLock;

/// Cached PATH from user's login shell, refreshable on tool-not-found.
static DISCOVERED_PATH: OnceLock<Mutex<String>> = OnceLock::new();

/// Try to discover PATH by spawning a login shell with the given binary.
///
/// `interactive` decides which startup files the shell reads. zsh sources
/// `.zshrc` only for interactive shells, and that is where PATH additions
/// commonly live, so a non-interactive login shell can report a PATH the
/// user's own terminal does not have. It is also the faster of the two, so
/// startup uses it and the retry pays for the interactive one.
fn try_shell(shell: &str, interactive: bool) -> Result<String, String> {
    let flags = if interactive { "-ilc" } else { "-lc" };
    let output = std::process::Command::new(shell)
        .args([flags, "echo $PATH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to spawn {}: {}", shell, e))?;

    if !output.status.success() {
        return Err(format!("{} exited with non-zero status", shell));
    }

    // An interactive shell may greet before it answers, so take the last
    // non-empty line and require it to look like a PATH rather than a prompt.
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()
        .unwrap_or_default()
        .to_string();
    if !path.split(':').any(|entry| entry == "/usr/bin") {
        return Err(format!("{} did not return a usable PATH", shell));
    }

    Ok(path)
}

/// Spawn the user's login shell to get their full PATH.
/// Tries $SHELL first, then falls back to /bin/zsh and /bin/bash.
fn discover_path_from_shell(interactive: bool) -> Result<String, String> {
    // Try the user's configured shell first
    if let Ok(shell) = std::env::var("SHELL") {
        if let Ok(path) = try_shell(&shell, interactive) {
            return Ok(path);
        }
    }

    // Fallback: try common shells
    for shell in &["/bin/zsh", "/bin/bash"] {
        if let Ok(path) = try_shell(shell, interactive) {
            return Ok(path);
        }
    }

    Err("All shell attempts failed".to_string())
}

/// Discover the user's PATH and set it process-wide.
/// Call once at GUI startup. Falls back to common paths if discovery fails.
pub fn init_path() {
    let path = match discover_path_from_shell(false) {
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

/// Re-discover PATH and update the process environment.
///
/// Called when a tool looks missing. Uses an interactive shell, which reads
/// the startup files a plain login shell skips, so it sees what the terminal
/// twapp spawns will see.
pub fn refresh_path() -> Result<(), String> {
    let path = discover_path_from_shell(true)?;
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

#[cfg(test)]
mod path_discovery_tests {
    use super::*;

    /// zsh reads .zshrc only when interactive, and PATH additions commonly
    /// live there. A non-interactive login shell therefore reports a PATH the
    /// user's own terminal does not have, which is what made an installed
    /// harness look missing.
    #[test]
    fn an_interactive_shell_sees_path_entries_a_login_shell_misses() {
        let dir = std::env::temp_dir().join(format!("twapp-zdot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zshrc"), "export PATH=\"/zshrc-only:$PATH\"\n").unwrap();
        std::fs::write(dir.join(".zprofile"), "export PATH=\"/zprofile-only:$PATH\"\n").unwrap();

        let run = |interactive: bool| {
            let flags = if interactive { "-ilc" } else { "-lc" };
            let out = std::process::Command::new("/bin/zsh")
                .args([flags, "echo $PATH"])
                .env("ZDOTDIR", &dir)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };

        assert!(run(false).contains("/zprofile-only"));
        assert!(!run(false).contains("/zshrc-only"));
        assert!(run(true).contains("/zshrc-only"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shell_that_greets_before_answering_still_yields_its_path() {
        // try_shell takes the last non-empty line, so a banner printed by an
        // interactive startup file does not become the PATH.
        let dir = std::env::temp_dir().join(format!("twapp-zdot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zshrc"), "echo 'welcome to the shell'\n").unwrap();
        std::env::set_var("ZDOTDIR", &dir);

        let path = try_shell("/bin/zsh", true).unwrap();

        assert!(!path.contains("welcome"), "{}", path);
        assert!(path.split(':').any(|entry| entry == "/usr/bin"), "{}", path);

        std::env::remove_var("ZDOTDIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
