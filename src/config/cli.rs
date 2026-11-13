//! CLI argument parsing: the flags and subcommands the binary accepts.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use super::ProviderKind;

#[derive(Debug, Parser)]
#[command(name = "yopes", version = crate::update::current_version(), about = "A fast, session-first AI harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Model backend: openai or ollama.
    #[arg(long, value_enum)]
    pub(super) provider: Option<ProviderKind>,

    /// Model name sent to the provider.
    #[arg(long)]
    pub(super) model: Option<String>,

    /// Provider base URL, without /chat/completions or /api/chat.
    #[arg(long)]
    pub(super) base_url: Option<String>,

    /// Session JSON path. Defaults to the platform data directory.
    #[arg(long)]
    pub(super) session: Option<PathBuf>,

    /// Do not load or save the session.
    #[arg(long)]
    pub(super) no_save: bool,

    /// Custom system prompt text, appended to the built-in one.
    #[arg(long)]
    pub(super) system_prompt: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check for updates, install the latest release, and restart.
    Update,
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use crate::config::{AuthKind, ProviderKind};
    use clap::Parser;

    #[test]
    fn provider_values_are_stable_cli_names() {
        let cli = Cli::try_parse_from(["yopes", "--provider", "openai"]).unwrap();
        assert_eq!(cli.provider, Some(ProviderKind::OpenAi));
        assert!(Cli::try_parse_from(["yopes", "--provider", "invalid"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "custom"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "demo"]).is_err());
    }

    #[test]
    fn custom_auth_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&AuthKind::XApiKey).unwrap(),
            "\"x-api-key\""
        );
        assert_eq!(
            serde_json::from_str::<AuthKind>("\"none\"").unwrap(),
            AuthKind::None
        );
    }

    #[test]
    fn the_update_subcommand_parses() {
        let cli = Cli::try_parse_from(["yopes", "update"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Update)));
    }
}
