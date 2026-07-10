use anyhow::{anyhow, Context};

use glossa_platform_windows::updater::run_local_updater;

pub async fn run() -> anyhow::Result<()> {
    let status = run_local_updater().context("failed to start the local updater")?;

    if status.success() {
        return Ok(());
    }

    Err(anyhow!("updater exited with status {status}"))
}
