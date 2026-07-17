use anyhow::Context;
use tracing::info;

use glossa_platform_windows::setup::{run_setup_wizard, WizardOutcome};

/// Runs the interactive first-run setup wizard.
pub async fn run() -> anyhow::Result<()> {
    info!("starting Glossa setup wizard");
    let outcome = tokio::task::spawn_blocking(run_setup_wizard)
        .await
        .context("setup wizard task failed")?
        .context("setup wizard failed")?;

    match outcome {
        WizardOutcome::Completed(result) => {
            println!("Installed Glossa to {}", result.install_dir.display());
            println!("Config: {}", result.config_path.display());
            if result.autostart {
                println!("Autostart: enabled (Startup folder)");
            }
            if result.start_now {
                println!("Daemon: started");
            } else {
                println!(
                    "Start with:\n  \"{}\" --config \"{}\" daemon",
                    result.exe_path.display(),
                    result.config_path.display()
                );
            }
            Ok(())
        }
        WizardOutcome::Cancelled => {
            println!("Setup cancelled.");
            Ok(())
        }
    }
}
