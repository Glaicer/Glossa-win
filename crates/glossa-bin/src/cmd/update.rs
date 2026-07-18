use anyhow::{anyhow, Context};

use glossa_platform_windows::updater::{install_update, UpdaterStatus};

pub async fn run() -> anyhow::Result<()> {
    let result = install_update().context("failed to run the local updater")?;

    match result.status {
        UpdaterStatus::Updated => {
            println!("Updated Glossa to {}.", result.version);
            Ok(())
        }
        UpdaterStatus::UpToDate => {
            println!("Glossa {} is already installed.", result.version);
            Ok(())
        }
        UpdaterStatus::Available => Err(anyhow!(
            "updater reported an available update without installing"
        )),
    }
}
