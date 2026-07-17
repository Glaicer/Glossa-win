use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Command-line interface for the Glossa daemon and helper tools.
#[derive(Debug, Parser)]
#[command(
    name = "glossa",
    about = "Speech-to-text daemon for Windows",
    version
)]
pub struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Subcommand to run. When omitted, Glossa runs setup (first run) or the daemon.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level commands supported by the binary.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the speech-to-text daemon (starts the first-run wizard when no config exists).
    Daemon,
    /// Interactive first-run installer: API key, hotkey, assets, optional autostart.
    Setup,
    Service {
        #[command(subcommand)]
        service: ServiceCommand,
    },
    Ctl {
        #[command(subcommand)]
        ctl: CtlCommand,
    },
    Doctor,
    Status,
    Update,
}

/// Subcommands for the external control CLI.
#[derive(Debug, Subcommand)]
pub enum CtlCommand {
    Toggle,
    Stream,
    Shutdown,
}

/// Subcommands for managing Glossa autostart on Windows.
#[derive(Debug, Clone, Copy, Subcommand, PartialEq, Eq)]
pub enum ServiceCommand {
    Start,
    Stop,
    Restart,
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;

    use super::{Cli, ServiceCommand};

    #[test]
    fn config_flag_after_subcommand_should_parse() {
        let cli = Cli::try_parse_from(["glossa", "daemon", "--config", r"C:\glossa\config.toml"])
            .expect("config flag after daemon subcommand should parse");

        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new(r"C:\glossa\config.toml"))
        );
    }

    #[test]
    fn config_flag_before_subcommand_should_parse() {
        let cli = Cli::try_parse_from(["glossa", "--config", r"C:\glossa\config.toml", "daemon"])
            .expect("config flag before daemon subcommand should parse");

        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new(r"C:\glossa\config.toml"))
        );
    }

    #[test]
    fn bare_invocation_should_parse_with_no_subcommand() {
        let cli = Cli::try_parse_from(["glossa"]).expect("bare glossa should parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn setup_subcommand_should_parse() {
        let cli = Cli::try_parse_from(["glossa", "setup"]).expect("setup should parse");
        assert!(matches!(cli.command, Some(super::Command::Setup)));
    }

    #[test]
    fn update_subcommand_should_parse() {
        let cli =
            Cli::try_parse_from(["glossa", "update"]).expect("update subcommand should parse");

        assert!(matches!(cli.command, Some(super::Command::Update)));
    }

    #[test]
    fn service_start_subcommand_should_parse() {
        let cli = Cli::try_parse_from(["glossa", "service", "start"])
            .expect("service start subcommand should parse");

        assert!(matches!(
            cli.command,
            Some(super::Command::Service {
                service: ServiceCommand::Start
            })
        ));
    }

    #[test]
    fn ctl_stream_subcommand_should_parse() {
        let cli =
            Cli::try_parse_from(["glossa", "ctl", "stream"]).expect("ctl stream should parse");

        assert!(matches!(
            cli.command,
            Some(super::Command::Ctl {
                ctl: super::CtlCommand::Stream
            })
        ));
    }

    #[test]
    fn version_flag_should_be_available() {
        let error = Cli::try_parse_from(["glossa", "--version"])
            .expect_err("version flag should short-circuit parsing");

        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
    }
}
