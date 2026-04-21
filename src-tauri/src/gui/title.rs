/// Co-lab role archetypes — these are the roles we render as
/// `co-lab:<role>` in the OS window title. Other non-empty role strings
/// are permitted on `SessionData.role` (the field is free-form), but
/// they collapse to the plain `co-lab` prefix if provenance is spawned,
/// and to no prefix at all if provenance is user. The list mirrors the
/// archetypes called out by the co-lab docs-brand briefing.
pub const COLAB_ROLE_ARCHETYPES: &[&str] = &[
    "coordinator",
    "implementer",
    "reviewer",
    "auditor",
    "log-watcher",
    "architect",
    "qa",
    "area-owner",
    "designer",
];

fn is_colab_archetype(role: &str) -> bool {
    COLAB_ROLE_ARCHETYPES.contains(&role)
}

/// Compose the OS window title for a twapp instance.
///
/// Precedence:
/// 1. `role` is a known co-lab archetype → `twapp - co-lab:<role> - <name>`
/// 2. `provenance == "spawned"`         → `twapp - co-lab - <name>`
/// 3. otherwise                          → unchanged `twapp` / `twapp - <name>`
///
/// The unchanged branch is what the app rendered before this function
/// existed; single-session plain usage must see zero regression.
pub fn format_window_title(
    name: &str,
    role: Option<&str>,
    provenance: Option<&str>,
) -> String {
    let plain = if name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", name)
    };

    if let Some(r) = role {
        if is_colab_archetype(r) {
            return format!("twapp - co-lab:{} - {}", r, name);
        }
    }
    if provenance == Some("spawned") {
        return format!("twapp - co-lab - {}", name);
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_user_session_unchanged() {
        assert_eq!(
            format_window_title("bash", None, Some("user")),
            "twapp - bash"
        );
    }

    #[test]
    fn plain_user_session_no_provenance_unchanged() {
        // Legacy session files predate the provenance field.
        assert_eq!(
            format_window_title("legacy", None, None),
            "twapp - legacy"
        );
    }

    #[test]
    fn default_instance_name_stays_bare() {
        // The GUI falls back to `name == "twapp"` when nothing was passed;
        // that case must still produce just `twapp` with no trailing dash.
        assert_eq!(format_window_title("twapp", None, None), "twapp");
    }

    #[test]
    fn spawned_without_role_gets_colab_prefix() {
        assert_eq!(
            format_window_title("my-worker", None, Some("spawned")),
            "twapp - co-lab - my-worker"
        );
    }

    #[test]
    fn coordinator_role_gets_role_scoped_prefix() {
        assert_eq!(
            format_window_title("my-coord", Some("coordinator"), Some("spawned")),
            "twapp - co-lab:coordinator - my-coord"
        );
    }

    #[test]
    fn reviewer_role_gets_role_scoped_prefix() {
        assert_eq!(
            format_window_title("feature-x", Some("reviewer"), Some("spawned")),
            "twapp - co-lab:reviewer - feature-x"
        );
    }

    #[test]
    fn archetype_role_wins_even_if_provenance_is_user() {
        // role set takes precedence — a reviewer deliberately launched by
        // a human should still be identifiable as co-lab chrome.
        assert_eq!(
            format_window_title("feature-x", Some("reviewer"), Some("user")),
            "twapp - co-lab:reviewer - feature-x"
        );
    }

    #[test]
    fn non_archetype_role_with_spawned_falls_through_to_plain_colab() {
        // Free-form role that isn't in the archetype list: we still want
        // *some* co-lab signal if the session was spawned, so it collapses
        // to the plain `co-lab` prefix rather than a role-scoped one.
        assert_eq!(
            format_window_title("oddjob", Some("mystery-role"), Some("spawned")),
            "twapp - co-lab - oddjob"
        );
    }

    #[test]
    fn non_archetype_role_with_user_provenance_stays_plain() {
        // Free-form non-archetype role + user provenance: no co-lab chrome.
        // This matches the briefing's strict ordering.
        assert_eq!(
            format_window_title("oddjob", Some("mystery-role"), Some("user")),
            "twapp - oddjob"
        );
    }

    #[test]
    fn all_archetypes_produce_role_scoped_prefix() {
        for archetype in COLAB_ROLE_ARCHETYPES {
            let title = format_window_title("x", Some(archetype), None);
            assert_eq!(title, format!("twapp - co-lab:{} - x", archetype));
        }
    }
}
