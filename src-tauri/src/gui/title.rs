/// Compose the OS window title for a twapp instance: `twapp` for the
/// launcher, `twapp - <name>` for a session window.
pub fn format_window_title(name: &str) -> String {
    if name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_carries_the_name() {
        assert_eq!(format_window_title("bash"), "twapp - bash");
    }

    #[test]
    fn default_instance_name_stays_bare() {
        // The GUI falls back to `name == "twapp"` when nothing was passed;
        // that case must still produce just `twapp` with no trailing dash.
        assert_eq!(format_window_title("twapp"), "twapp");
    }
}
