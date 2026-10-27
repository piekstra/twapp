//! `twapp relay`: the chat channels the user shares with other people's
//! agents, from the `relays:` list in `config.yaml`.

use super::config::{config_file, get_relays, Relay};

pub fn cmd_relay_list(json: bool) -> i32 {
    let relays = get_relays();
    if json {
        println!("{}", serde_json::to_string_pretty(&relays).unwrap_or_else(|_| "[]".to_string()));
        return 0;
    }
    if relays.is_empty() {
        println!("No relays configured. Add a relays: list to {}.", config_file().display());
        return 0;
    }
    for relay in &relays {
        print!("{}", describe(relay));
    }
    0
}

fn describe(relay: &Relay) -> String {
    let mut out = format!(
        "{}\n  {} workspace {}, channel {}\n",
        relay.name, relay.service, relay.workspace, relay.channel
    );
    if let Some(post_as) = &relay.post_as {
        out.push_str(&format!("  posts as {}\n", post_as));
    }
    if let Some(instructions) = &relay.instructions {
        out.push_str(&format!("  {}\n", instructions));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_a_relay_with_its_workspace_and_optional_fields() {
        let relay = Relay {
            name: "agents".to_string(),
            service: "slack".to_string(),
            workspace: "T0123".to_string(),
            channel: "C0456".to_string(),
            post_as: Some("my-bot".to_string()),
            instructions: None,
        };
        assert_eq!(describe(&relay), "agents\n  slack workspace T0123, channel C0456\n  posts as my-bot\n");
    }
}
