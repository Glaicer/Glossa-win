mod bootstrap;
mod cli;
mod cmd;

use anyhow::Context;
use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Credential Manager backend for `cred:NAME` secret sources (and doctor/settings).
    glossa_platform_windows::credentials::install_default_store();

    let cli = Cli::parse();

    match cli.command {
        None => cmd::daemon::run_auto(cli.config).await,
        Some(Command::Daemon) => cmd::daemon::run(cli.config).await,
        Some(Command::Setup) => cmd::setup::run().await,
        Some(Command::Service { service }) => cmd::service::run(service).await,
        Some(Command::Ctl { ctl }) => cmd::ctl::run(cli.config, ctl).await,
        Some(Command::Doctor) => cmd::doctor::run(cli.config).await,
        Some(Command::Status) => cmd::status::run(cli.config).await,
        Some(Command::Update) => cmd::update::run().await,
    }
    .context("command failed")
}
