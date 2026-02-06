use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gui::{extract_adf_text, truncate_str};

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

/// Fetch a Jira ticket using jtk CLI and return normalized ticket info.
pub fn fetch_jira_ticket(ticket_key: &str) -> Option<TicketInfo> {
    let result = std::process::Command::new("jtk")
        .args(["issues", "get", ticket_key, "-o", "json"])
        .output();

    let output = match result {
        Ok(o) => o,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("Error: 'jtk' (jira-ticket-cli) not found. Install it first.");
            } else {
                eprintln!("Error fetching Jira ticket: {}", e);
            }
            return None;
        }
    };

    if !output.status.success() {
        eprintln!(
            "Error fetching Jira ticket: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }

    let data: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing Jira response: {}", e);
            return None;
        }
    };

    // jtk may return an array or a single object
    let issue = if let Some(arr) = data.as_array() {
        arr.first()?.clone()
    } else {
        data
    };

    let fields = issue.get("fields")?;

    // Extract description as plain text from ADF
    let description_text = fields
        .get("description")
        .filter(|d| !d.is_null())
        .map(|d| extract_adf_text(d));

    // Extract story points from description (e.g., "Story Points: 3")
    let points = description_text.as_ref().and_then(|text| {
        let lower = text.to_lowercase();
        let idx = lower.find("story point")?;
        let after = &text[idx..];
        // Skip past "Story Point(s):" and whitespace/colons
        let digits_start = after.find(|c: char| c.is_ascii_digit())?;
        let digits: String = after[digits_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            None
        } else {
            Some(digits)
        }
    });

    // Epic info from parent
    let epic = issue
        .get("fields")
        .and_then(|f| f.get("parent"))
        .and_then(|parent| {
            let key = parent.get("key")?.as_str()?;
            let summary = parent.get("fields")?.get("summary")?.as_str()?;
            Some(format!("{}: {}", key, summary))
        });

    // Jira base URL from self link
    let base_url = issue
        .get("self")
        .and_then(|s| s.as_str())
        .and_then(|url| {
            let idx = url.find("/rest/")?;
            Some(url[..idx].to_string())
        });

    let key = issue
        .get("key")
        .and_then(|k| k.as_str())
        .unwrap_or(ticket_key)
        .to_string();

    let url = base_url.map(|base| format!("{}/browse/{}", base, key));

    Some(TicketInfo {
        source: "jira".to_string(),
        key,
        title: fields
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        r#type: fields
            .get("issuetype")
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string(),
        status: fields
            .get("status")
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string(),
        priority: fields
            .get("priority")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(String::from),
        points,
        sprint: None,
        epic,
        assignee: fields
            .get("assignee")
            .and_then(|a| a.get("displayName"))
            .and_then(|n| n.as_str())
            .map(String::from),
        description: description_text.map(|t| truncate_str(&t, 2000)),
        url,
    })
}

/// Fetch a GitHub issue using gh CLI and return normalized ticket info.
/// identifier can be: #123, owner/repo#123, or just 123
pub fn fetch_github_issue(identifier: &str, default_repo: Option<&str>) -> Option<TicketInfo> {
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

    let repo = match repo {
        Some(r) => r,
        None => {
            eprintln!("Error: No GitHub repo specified. Use owner/repo#123 or set github_repo in config.");
            return None;
        }
    };

    let result = std::process::Command::new("gh")
        .args([
            "issue",
            "view",
            number,
            "--repo",
            repo,
            "--json",
            "title,body,state,labels,milestone,assignees,number,url",
        ])
        .output();

    let output = match result {
        Ok(o) => o,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("Error: 'gh' (GitHub CLI) not found. Install it first.");
            } else {
                eprintln!("Error fetching GitHub issue: {}", e);
            }
            return None;
        }
    };

    if !output.status.success() {
        eprintln!(
            "Error fetching GitHub issue: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }

    let data: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing GitHub response: {}", e);
            return None;
        }
    };

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

    Some(TicketInfo {
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
