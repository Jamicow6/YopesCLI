use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolsMode {
    #[default]
    On,
    Off,
    Auto,
    Ask,
}

impl ToolsMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Auto => "yolo",
            Self::Ask => "ask",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::On => "edits run automatically; commands the model flags as risky ask",
            Self::Off => "tools are not offered to the model",
            Self::Auto => "YOLO: all commands and edits run automatically without asking",
            Self::Ask => "every command the model flags as risky asks",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "on" | "normal" => Some(Self::On),
            "off" => Some(Self::Off),
            "auto" | "yolo" => Some(Self::Auto),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }

    pub const fn next_permission(self) -> Self {
        match self {
            Self::On => Self::Auto,
            Self::Auto => Self::Ask,
            Self::Ask => Self::On,
            Self::Off => Self::On,
        }
    }

    pub const fn advertised(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub(crate) fn needs_approval(self, call: &ToolCall, approved: &HashSet<String>) -> bool {
        match self {
            Self::Off | Self::Auto => false,
            Self::Ask => tools::requires_approval(call),
            Self::On => tools::requires_approval(call) && !approved.contains(&call.function.name),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    /// Apply changes: tools follow the /tools setting as usual.
    #[default]
    Build,
    /// Research and propose only; writes and commands are blocked.
    Plan,
    /// Plain conversation; tools are not offered or run.
    Chat,
}

impl Mode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Plan => "plan",
            Self::Chat => "chat",
        }
    }

    /// The colour of the mode's dot in the header.
    ///
    /// Build is purple, chat is a yellow-brown, and plan is a brighter brown,
    /// so the mode reads at a glance without reading the word.
    pub const fn color(self) -> ratatui::style::Color {
        match self {
            Self::Build => ratatui::style::Color::Rgb(175, 122, 255),
            Self::Chat => ratatui::style::Color::Rgb(214, 168, 84),
            Self::Plan => ratatui::style::Color::Rgb(196, 132, 90),
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Build => Self::Plan,
            Self::Plan => Self::Chat,
            Self::Chat => Self::Build,
        }
    }

    /// Tool names the model may not call in this mode.
    pub(crate) fn denied(self, tool: &str) -> bool {
        match self {
            Self::Build => false,
            Self::Plan => {
                matches!(
                    tool,
                    "write_file" | "edit_file" | "multi_edit" | "run_command" | "bash_background"
                )
            }
            Self::Chat => true,
        }
    }

    /// An instruction appended to the system prompt for this turn.
    pub(crate) fn instruction(self, tools_mode: ToolsMode) -> Option<&'static str> {
        match self {
            // Build also states its mode: without a note, nothing tells the
            // model that plan mode ended, and it keeps gating itself.
            Self::Build => match tools_mode {
                ToolsMode::Auto => Some(
                    "BUILD MODE (YOLO): changes may be applied directly. All commands and \
                     tool calls are pre-approved by the user. Run risky, destructive, or \
                     modifying commands (including file deletion, git resets, and package \
                     installations) immediately via tools without asking for confirmation \
                     or permission. Never end a response asking if you should run a \
                     command — execute it directly.",
                ),
                _ => Some(
                    "BUILD MODE: changes may be applied. Use the tools to write files, edit files, \
                     and run commands as the user asked, subject to the approval rules above.",
                ),
            },
            Self::Plan => Some(
                "PLAN MODE: research and propose a plan only. Do not write files, edit files, \
                 or run commands; the user will switch to build mode to apply changes. Reading \
                 files, listing directories, and web search stay allowed.",
            ),
            Self::Chat => Some("CHAT MODE: tools are disabled. Answer in plain text."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, ToolsMode};

    #[test]
    fn the_mode_cycle_and_denials_match_the_labels() {
        assert_eq!(Mode::default(), Mode::Build);
        assert_eq!(Mode::Build.next(), Mode::Plan);
        assert_eq!(Mode::Plan.next(), Mode::Chat);
        assert_eq!(Mode::Chat.next(), Mode::Build);
        assert!(!Mode::Build.denied("write_file"));
        assert!(!Mode::Plan.denied("read_file"));
        assert!(!Mode::Plan.denied("web_search"));
        assert!(Mode::Plan.denied("write_file"));
        assert!(Mode::Plan.denied("edit_file"));
        assert!(Mode::Plan.denied("multi_edit"));
        assert!(Mode::Plan.denied("run_command"));
        assert!(Mode::Plan.denied("bash_background"));
        assert!(Mode::Chat.denied("read_file"));
        assert!(
            Mode::Plan
                .instruction(ToolsMode::On)
                .is_some_and(|note| note.contains("PLAN"))
        );
        assert!(
            Mode::Build
                .instruction(ToolsMode::On)
                .is_some_and(|note| note.contains("BUILD"))
        );
        assert!(
            Mode::Build
                .instruction(ToolsMode::Auto)
                .is_some_and(|note| note.contains("YOLO"))
        );
        assert_eq!(ToolsMode::On.next_permission(), ToolsMode::Auto);
        assert_eq!(ToolsMode::Auto.next_permission(), ToolsMode::Ask);
        assert_eq!(ToolsMode::Ask.next_permission(), ToolsMode::On);
        assert_eq!(ToolsMode::Off.next_permission(), ToolsMode::On);
    }
}
