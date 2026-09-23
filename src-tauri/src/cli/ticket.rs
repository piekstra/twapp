use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::GlobalConfig;
use crate::gui::shell_env::{run_tool_sync, TOOL_GH, TOOL_JTK};
use crate::gui::truncate_str;

#[derive(Debug, Serialize, Deserialize)]
pub struct TicketInfo {
    pub source: String,
    pub key: String,
    pub title: String,
    pub r#type: String,
    pub status: String,
    pub priority: Option<String>,
    pub points: Option<String>,
    pub sprint: Option<String>,
    pub epic: Option<String>,
    pub assignee: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
}

/// Fields requested from `jtk issues get`. Description stays last because
/// `--fulltext` lets it span lines, so everything after its label belongs to it.
const JTK_FIELDS: &str =
    "Summary,Status,Issue Type,Priority,Assignee,Parent,Sprint,Story Points,Description";

const DESCRIPTION_LIMIT: usize = 2000;

static JIRA_BASE_URL: Mutex<Option<String>> = Mutex::new(None);

/// A ticket reference as typed by the user, classified the same way for the
/// CLI and the GUI.
#[derive(Debug, PartialEq, Eq)]
pub enum TicketRef {
    Jira(String),
    GitHub(String),
}

/// `#` (or `force_github`) means a GitHub issue; a bare number is prefixed
/// with the configured Jira project; anything else is a Jira key as typed.
pub fn classify_ticket_ref(
    input: &str,
    jira_project: Option<&str>,
    force_github: bool,
) -> TicketRef {
    let input = input.trim();
    if force_github || input.contains('#') {
        return TicketRef::GitHub(input.to_string());
    }
    match jira_project {
        Some(project) if !input.is_empty() && input.chars().all(|c| c.is_ascii_digit()) => {
            TicketRef::Jira(format!("{}-{}", project, input))
        }
        _ => TicketRef::Jira(input.to_string()),
    }
}

/// Fetch any ticket reference, reading the Jira project and GitHub repo
/// defaults from the global config.
pub fn fetch_ticket(input: &str, force_github: bool) -> Result<TicketInfo, String> {
    let config = GlobalConfig::load().ok();
    let jira_project = config.as_ref().and_then(|c| c.jira_project.as_deref());
    match classify_ticket_ref(input, jira_project, force_github) {
        TicketRef::GitHub(identifier) => fetch_github_issue(
            &identifier,
            config.as_ref().and_then(|c| c.github_repo.as_deref()),
        ),
        TicketRef::Jira(key) => fetch_jira_ticket(&key),
    }
}

/// Re-fetch a previously linked ticket from the source it was linked from.
pub fn refresh_ticket_info(old: &TicketInfo) -> Result<TicketInfo, String> {
    match old.source.as_str() {
        "github" => {
            let config = GlobalConfig::load().ok();
            fetch_github_issue(&old.key, config.as_ref().and_then(|c| c.github_repo.as_deref()))
        }
        _ => fetch_jira_ticket(&old.key),
    }
}

/// Fetch a Jira ticket using jtk CLI and return normalized ticket info.
pub fn fetch_jira_ticket(ticket_key: &str) -> Result<TicketInfo, String> {
    let output = run_tool_sync(
        &TOOL_JTK,
        &[
            "issues",
            "get",
            ticket_key,
            "--no-color",
            "--fulltext",
            "--fields",
            JTK_FIELDS,
        ],
    )?;

    if !output.status.success() {
        return Err(format!(
            "Error fetching Jira ticket: {}",
            jtk_error(&String::from_utf8_lossy(&output.stderr))
        ));
    }

    let ticket = parse_jtk_issue(
        &String::from_utf8_lossy(&output.stdout),
        ticket_key,
        jira_base_url().as_deref(),
    );
    if ticket.title.is_empty() && ticket.status.is_empty() {
        return Err(format!(
            "Error parsing Jira response for {}: no Summary or Status in jtk output",
            ticket_key
        ));
    }
    Ok(ticket)
}

/// Turn a jtk failure into a message. An unknown flag means the installed jtk
/// does not speak the text output twapp reads.
pub fn jtk_error(stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.contains("unknown flag") || stderr.contains("unknown shorthand flag") {
        format!(
            "incompatible jtk version (twapp needs jtk 1.3+ text output): {}",
            stderr
        )
    } else {
        stderr.to_string()
    }
}

fn field_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value.to_string())
    }
}

/// Parse the `Label: value` block printed by `jtk issues get --fields`.
pub fn parse_jtk_issue(stdout: &str, key_hint: &str, base_url: Option<&str>) -> TicketInfo {
    let mut key = None;
    let mut title = None;
    let mut status = None;
    let mut issue_type = None;
    let mut priority = None;
    let mut assignee = None;
    let mut parent = None;
    let mut sprint = None;
    let mut points = None;
    let mut description: Option<String> = None;

    for line in stdout.lines() {
        if let Some(text) = description.as_mut() {
            text.push('\n');
            text.push_str(line);
            continue;
        }
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        match label.trim() {
            "Key" => key = field_value(value),
            "Summary" => title = field_value(value),
            "Status" => status = field_value(value),
            "Type" => issue_type = field_value(value),
            "Priority" => priority = field_value(value),
            "Assignee" => assignee = field_value(value),
            "Parent" => parent = field_value(value),
            "Sprint" => sprint = field_value(value),
            "Points" => points = field_value(value),
            "Description" => description = Some(value.trim_start().to_string()),
            _ => {}
        }
    }

    let key = key.unwrap_or_else(|| key_hint.to_string());
    let epic = parent.map(|p| {
        match p.split_once(" \u{2014} ").or_else(|| p.split_once(" - ")) {
            Some((parent_key, summary)) => format!("{}: {}", parent_key.trim(), summary.trim()),
            None => p,
        }
    });
    let url = base_url
        .map(|base| base.trim_end_matches('/'))
        .filter(|base| !base.is_empty())
        .map(|base| format!("{}/browse/{}", base, key));

    TicketInfo {
        source: "jira".to_string(),
        key,
        title: title.unwrap_or_default(),
        r#type: issue_type.unwrap_or_default(),
        status: status.unwrap_or_default(),
        priority,
        points,
        sprint,
        epic,
        assignee,
        description: description
            .and_then(|d| field_value(&d))
            .map(|d| truncate_str(&d, DESCRIPTION_LIMIT)),
        url,
    }
}

/// Read the site URL from the `url` row of `jtk config show`.
pub fn parse_jtk_config_url(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut cells = line.split('|').map(str::trim);
        if cells.next()? != "url" {
            return None;
        }
        cells
            .next()
            .filter(|url| url.starts_with("http"))
            .map(|url| url.trim_end_matches('/').to_string())
    })
}

/// The Jira site used to build browse links: `jira_base_url` from the global
/// config, otherwise the site jtk is configured for.
pub fn jira_base_url() -> Option<String> {
    if let Some(url) = GlobalConfig::load().ok().and_then(|c| c.jira_base_url) {
        return Some(url.trim_end_matches('/').to_string());
    }
    if let Some(url) = JIRA_BASE_URL.lock().clone() {
        return Some(url);
    }
    let output = run_tool_sync(&TOOL_JTK, &["config", "show", "--no-color"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let url = parse_jtk_config_url(&String::from_utf8_lossy(&output.stdout))?;
    *JIRA_BASE_URL.lock() = Some(url.clone());
    Some(url)
}

/// Create a Jira ticket and return its key.
pub fn create_jira_ticket(
    project: &str,
    summary: &str,
    issue_type: &str,
) -> Result<String, String> {
    let output = run_tool_sync(
        &TOOL_JTK,
        &[
            "issues",
            "create",
            "--project",
            project,
            "--summary",
            summary,
            "--type",
            issue_type,
            "--id",
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "Error creating ticket: {}",
            jtk_error(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Error: Could not parse created ticket key from jtk output".to_string())
}

/// Fetch a GitHub issue using gh CLI and return normalized ticket info.
/// identifier can be: #123, owner/repo#123, or just 123
pub fn fetch_github_issue(identifier: &str, default_repo: Option<&str>) -> Result<TicketInfo, String> {
    let (repo, number) = if identifier.contains('#') {
        let parts: Vec<&str> = identifier.splitn(2, '#').collect();
        let repo = if parts[0].is_empty() {
            default_repo
        } else {
            Some(parts[0])
        };
        (repo, parts[1])
    } else {
        (default_repo, identifier)
    };

    let repo = repo.ok_or_else(|| {
        "Error: No GitHub repo specified. Use owner/repo#123 or set github_repo in config."
            .to_string()
    })?;

    let output = run_tool_sync(
        &TOOL_GH,
        &[
            "issue",
            "view",
            number,
            "--repo",
            repo,
            "--json",
            "title,body,state,labels,milestone,assignees,number,url",
        ],
    )?;

    if !output.status.success() {
        return Err(format!(
            "Error fetching GitHub issue: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let data: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Error parsing GitHub response: {}", e))?;

    let issue_number = data
        .get("number")
        .and_then(|n| n.as_u64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| number.to_string());

    let labels: Vec<String> = data
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let assignee = data
        .get("assignees")
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
        .map(String::from);

    let body = data
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .to_string();

    Ok(TicketInfo {
        source: "github".to_string(),
        key: format!("{}#{}", repo, issue_number),
        title: data
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        r#type: "Issue".to_string(),
        status: data
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        priority: if labels.is_empty() {
            None
        } else {
            Some(labels.join(", "))
        },
        points: None,
        sprint: data
            .get("milestone")
            .and_then(|m| m.get("title"))
            .and_then(|t| t.as_str())
            .map(String::from),
        epic: None,
        assignee,
        description: if body.is_empty() {
            None
        } else {
            Some(truncate_str(&body, 2000))
        },
        url: data
            .get("url")
            .and_then(|u| u.as_str())
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUE_FULL: &str = include_str!("../../tests/fixtures/jtk/issue_get_full.txt");
    const ISSUE_SPARSE: &str = include_str!("../../tests/fixtures/jtk/issue_get_sparse.txt");
    const CONFIG_SHOW: &str = include_str!("../../tests/fixtures/jtk/config_show.txt");

    #[test]
    fn parses_every_jtk_field() {
        let ticket = parse_jtk_issue(ISSUE_FULL, "ABC-123", Some("https://example.atlassian.net"));
        assert_eq!(ticket.source, "jira");
        assert_eq!(ticket.key, "ABC-123");
        assert_eq!(ticket.title, "Add retry: backoff to the export job");
        assert_eq!(ticket.status, "In Progress");
        assert_eq!(ticket.r#type, "Story");
        assert_eq!(ticket.priority.as_deref(), Some("High"));
        assert_eq!(ticket.assignee.as_deref(), Some("Pat Example"));
        assert_eq!(ticket.epic.as_deref(), Some("ABC-100: Parent epic"));
        assert_eq!(ticket.sprint.as_deref(), Some("ABC Sprint 12"));
        assert_eq!(ticket.points.as_deref(), Some("3"));
        assert_eq!(
            ticket.url.as_deref(),
            Some("https://example.atlassian.net/browse/ABC-123")
        );
    }

    #[test]
    fn description_keeps_every_line_after_its_label() {
        let ticket = parse_jtk_issue(ISSUE_FULL, "ABC-123", None);
        let description = ticket.description.unwrap();
        assert!(description.starts_with("The export job gives up after one failure.\nNote: "));
        assert!(description.ends_with("- Status: should not be read as a field."));
        assert_eq!(ticket.status, "In Progress");
    }

    #[test]
    fn dash_placeholders_become_none() {
        let ticket = parse_jtk_issue(ISSUE_SPARSE, "ABC-124", None);
        assert_eq!(ticket.title, "Tidy the settings page");
        assert_eq!(ticket.priority, None);
        assert_eq!(ticket.assignee, None);
        assert_eq!(ticket.epic, None);
        assert_eq!(ticket.sprint, None);
        assert_eq!(ticket.points, None);
        assert_eq!(ticket.description, None);
        assert_eq!(ticket.url, None);
    }

    #[test]
    fn parent_accepts_a_plain_hyphen_separator() {
        let ticket = parse_jtk_issue("Key: ABC-1\nParent: ABC-2 - Epic title\n", "ABC-1", None);
        assert_eq!(ticket.epic.as_deref(), Some("ABC-2: Epic title"));
    }

    #[test]
    fn missing_key_falls_back_to_the_requested_key() {
        let ticket = parse_jtk_issue("Summary: Something\n", "ABC-9", Some("https://example.atlassian.net/"));
        assert_eq!(ticket.key, "ABC-9");
        assert_eq!(
            ticket.url.as_deref(),
            Some("https://example.atlassian.net/browse/ABC-9")
        );
    }

    #[test]
    fn reads_site_url_from_jtk_config_show() {
        assert_eq!(
            parse_jtk_config_url(CONFIG_SHOW).as_deref(),
            Some("https://example.atlassian.net")
        );
        assert_eq!(parse_jtk_config_url("url |  | -\n"), None);
    }

    #[test]
    fn classifies_ticket_refs_like_the_cli() {
        assert_eq!(
            classify_ticket_ref("123", Some("ABC"), false),
            TicketRef::Jira("ABC-123".to_string())
        );
        assert_eq!(
            classify_ticket_ref(" ABC-7 ", Some("ABC"), false),
            TicketRef::Jira("ABC-7".to_string())
        );
        assert_eq!(
            classify_ticket_ref("123", None, false),
            TicketRef::Jira("123".to_string())
        );
        assert_eq!(
            classify_ticket_ref("owner/repo#12", Some("ABC"), false),
            TicketRef::GitHub("owner/repo#12".to_string())
        );
        assert_eq!(
            classify_ticket_ref("12", Some("ABC"), true),
            TicketRef::GitHub("12".to_string())
        );
    }

    #[test]
    fn unknown_flag_errors_name_the_jtk_version() {
        assert!(jtk_error("unknown shorthand flag: 'o' in -o\n").starts_with("incompatible jtk version"));
        assert_eq!(jtk_error("issue does not exist\n"), "issue does not exist");
    }
}
