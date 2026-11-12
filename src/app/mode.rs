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
            Self::Auto => "auto",
            Self::Ask => "ask",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::On => "edits run automatically; commands the model flags as risky ask",
            Self::Off => "tools are not offered to the model",
            Self::Auto => "all tool calls run automatically",
            Self::Ask => "every command the model flags as risky asks",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "ask" => Some(Self::Ask),
            _ => None,
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
    pub(crate) fn instruction(self) -> Option<&'static str> {
        match self {
            // Build also states its mode: without a note, nothing tells the
            // model that plan mode ended, and it keeps gating itself.
            Self::Build => Some(
                "BUILD MODE: changes may be applied. Use the tools to write files, edit files, \
                 and run commands as the user asked, subject to the approval rules above.",
            ),
            Self::Plan => Some(
                "PLAN MODE: research and propose a plan only. Do not write files, edit files, \
                 or run commands; the user will switch to build mode to apply changes. Reading \
                 files, listing directories, and web search stay allowed.",
            ),
            Self::Chat => Some("CHAT MODE: tools are disabled. Answer in plain text."),
        }
    }
}
