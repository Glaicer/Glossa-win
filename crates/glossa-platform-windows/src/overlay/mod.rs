//! Best-effort recording and processing indicator drawn over the desktop.

mod overlay_port;
mod pill;
mod update_overlay;

pub use self::overlay_port::BestEffortOverlayPort;
pub use self::update_overlay::UpdateOverlay;
