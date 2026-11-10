use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use directories::ProjectDirs;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ProviderKind {
    Demo,
    #[value(name = "openai")]
    OpenAi,
    Ollama,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Demo => "demo",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
        }
    }

    fn parse_env(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "demo" => Ok(Self::Demo),
            "openai" | "openai-compatible" => Ok(Self::OpenAi),
            "ollama" => Ok(Self::Ollama),
            _ => bail!("invalid YOPES_PROVIDER `{value}`; expected demo, openai, or ollama"),
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "yopes", version, about = "A fast, session-first AI harness")]
struct Cli {
    /// Model backend: demo, openai, or ollama.
    #[arg(long, value_enum)]
    provider: Option<ProviderKind>,

    /// Model name sent to the provider.
    #[arg(long)]
    model: Option<String>,

    /// Provider base URL, without /chat/completions or /api/chat.
    #[arg(long)]
    base_url: Option<String>,

    /// Session JSON path. Defaults to the platform data directory.
    #[arg(long)]
    session: Option<PathBuf>,

    /// Do not load or save the session.
    #[arg(long)]
    no_save: bool,
}

pub struct Config {
    pub provider: ProviderKind,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub session_path: Option<PathBuf>,
}

impl Config {
    pub fn from_args() -> Result<Self> {
        Self::resolve(Cli::parse())
    }

    fn resolve(cli: Cli) -> Result<Self> {
        let env_provider = env::var("YOPES_PROVIDER")
            .ok()
            .map(|value| ProviderKind::parse_env(&value))
            .transpose()?;
        let api_key = env::var("OPENAI_API_KEY").ok();
        let provider = cli
            .provider
            .or(env_provider)
            .or_else(|| api_key.as_ref().map(|_| ProviderKind::OpenAi))
            .unwrap_or(ProviderKind::Demo);

        let model = cli
            .model
            .or_else(|| env::var("YOPES_MODEL").ok())
            .unwrap_or_else(|| {
                match provider {
                    ProviderKind::Demo => "demo",
                    ProviderKind::OpenAi => "gpt-4o-mini",
                    ProviderKind::Ollama => "llama3.2",
                }
                .to_owned()
            });
        if model.trim().is_empty() {
            bail!("model name cannot be empty");
        }

        let provider_base_var = match provider {
            ProviderKind::OpenAi => Some("OPENAI_BASE_URL"),
            ProviderKind::Ollama => Some("OLLAMA_BASE_URL"),
            ProviderKind::Demo => None,
        };
        let base_url = cli
            .base_url
            .or_else(|| env::var("YOPES_BASE_URL").ok())
            .or_else(|| provider_base_var.and_then(|name| env::var(name).ok()))
            .unwrap_or_else(|| match provider {
                ProviderKind::Demo => String::new(),
                ProviderKind::OpenAi => "https://api.openai.com/v1".to_owned(),
                ProviderKind::Ollama => "http://localhost:11434".to_owned(),
            });

        if provider == ProviderKind::OpenAi && api_key.as_deref().is_none_or(str::is_empty) {
            bail!("OPENAI_API_KEY is required for the OpenAI-compatible provider");
        }

        let session_path = if cli.no_save {
            None
        } else {
            Some(
                cli.session
                    .or_else(|| env::var("YOPES_SESSION").ok().map(PathBuf::from))
                    .unwrap_or_else(default_session_path),
            )
        };

        Ok(Self {
            provider,
            model,
            base_url,
            api_key,
            session_path,
        })
    }
}

fn default_session_path() -> PathBuf {
    ProjectDirs::from("dev", "Yopes", "Yopes").map_or_else(
        || PathBuf::from(".yopes-session.json"),
        |dirs| dirs.data_local_dir().join("session.json"),
    )
}

pub fn load_session(path: Option<&Path>) -> Result<crate::session::Session> {
    path.map_or_else(
        || Ok(crate::session::Session::new()),
        |path| {
            crate::session::Session::load(path)
                .with_context(|| format!("loading {}", path.display()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{Cli, ProviderKind};
    use clap::Parser;

    #[test]
    fn provider_values_are_stable_cli_names() {
        let cli = Cli::try_parse_from(["yopes", "--provider", "openai"]).unwrap();
        assert_eq!(cli.provider, Some(ProviderKind::OpenAi));
        assert!(Cli::try_parse_from(["yopes", "--provider", "invalid"]).is_err());
    }
}
