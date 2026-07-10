use anyhow::Context;

use glossa_platform_windows::service::{restart_autostart, start_autostart, stop_autostart};

use crate::cli::ServiceCommand;

pub async fn run(command: ServiceCommand) -> anyhow::Result<()> {
    match command {
        ServiceCommand::Start => start_autostart().context("failed to start Glossa autostart")?,
        ServiceCommand::Stop => stop_autostart().context("failed to stop Glossa autostart")?,
        ServiceCommand::Restart => {
            restart_autostart().context("failed to restart Glossa autostart")?
        }
    }
    println!("OK");
    Ok(())
}
