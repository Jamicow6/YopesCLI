//! The slash-command palette: the table the composer matches against and
//! the completion rules built from it.

pub(crate) const EFFORT_CHOICES: &str = "Usage: /effort off|none|low|medium|high|xhigh";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub argument: Option<&'static str>,
}

impl CommandSpec {
    pub fn display_name(self) -> String {
        self.argument.map_or_else(
            || self.name.to_owned(),
            |argument| format!("{} [{argument}]", self.name),
        )
    }

    pub(crate) fn completion(self) -> String {
        if self.argument.is_some() {
            format!("{} ", self.name)
        } else {
            self.name.to_owned()
        }
    }
}

pub const COMMANDS: [CommandSpec; 14] = [
    CommandSpec {
        name: "/help",
        description: "Show keys and commands",
        argument: None,
    },
    CommandSpec {
        name: "/new",
        description: "Start a fresh session",
        argument: None,
    },
    CommandSpec {
        name: "/connect",
        description: "Connect a provider and model",
        argument: None,
    },
    CommandSpec {
        name: "/disconnect",
        description: "Disconnect active or named provider",
        argument: Some("name"),
    },
    CommandSpec {
        name: "/cd",
        description: "Change the working directory",
        argument: Some("path"),
    },
    CommandSpec {
        name: "/models",
        description: "Browse models from connected providers",
        argument: Some("name"),
    },
    CommandSpec {
        name: "/sessions",
        description: "Switch between saved sessions",
        argument: None,
    },
    CommandSpec {
        name: "/tools",
        description: "Set tool mode: on, off, auto, ask (deprecated)",
        argument: Some("mode"),
    },
    CommandSpec {
        name: "/system",
        description: "Show or set the custom system prompt",
        argument: Some("text"),
    },
    CommandSpec {
        name: "/compact",
        description: "Compact history now; auto on|off",
        argument: Some("on|off|now"),
    },
    CommandSpec {
        name: "/effort",
        description: "Show or set reasoning effort",
        argument: Some("level"),
    },
    CommandSpec {
        name: "/skills",
        description: "List installed skills",
        argument: None,
    },
    CommandSpec {
        name: "/version",
        description: "Show the running version",
        argument: None,
    },
    CommandSpec {
        name: "/quit",
        description: "Save and exit",
        argument: None,
    },
];

pub(crate) fn matching_commands(value: &str) -> Vec<&'static CommandSpec> {
    if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = value[1..].to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| command.name[1..].starts_with(query.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, matching_commands};

    fn command_names(value: &str) -> Vec<&'static str> {
        matching_commands(value)
            .iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn palette_matches_command_prefixes() {
        assert_eq!(
            command_names("/"),
            vec![
                "/help",
                "/new",
                "/connect",
                "/disconnect",
                "/cd",
                "/models",
                "/sessions",
                "/tools",
                "/system",
                "/compact",
                "/effort",
                "/skills",
                "/version",
                "/quit",
            ]
        );
        assert_eq!(command_names("/c"), vec!["/connect", "/cd", "/compact"]);
        assert_eq!(command_names("/d"), vec!["/disconnect"]);
        assert_eq!(command_names("/m"), vec!["/models"]);
        assert_eq!(command_names("/t"), vec!["/tools"]);
        assert_eq!(command_names("/e"), vec!["/effort"]);
        assert_eq!(command_names("/v"), vec!["/version"]);
        assert_eq!(command_names("/MO"), vec!["/models"]);
        assert!(command_names("/model gpt").is_empty());
        assert!(command_names("hello").is_empty());
    }

    #[test]
    fn completion_adds_a_space_only_when_arguments_are_expected() {
        let model = COMMANDS
            .iter()
            .find(|command| command.name == "/models")
            .unwrap();
        assert_eq!(model.completion(), "/models ");
        let help = COMMANDS
            .iter()
            .find(|command| command.name == "/help")
            .unwrap();
        assert_eq!(help.completion(), "/help");
    }
}
