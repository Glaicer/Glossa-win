//! Windows-specific adapters for IPC, clipboard, paste, hotkeys, tray, and diagnostics.

pub mod clipboard;
pub mod console;
pub mod credentials;
pub mod dialog;
pub mod doctor;
pub mod hotkey;
pub mod ipc;
pub mod overlay;
pub mod paste;
pub mod service;
pub mod setup;
pub mod temp;
pub mod tray;
pub mod updater;
#[cfg(windows)]
mod win_style;
mod winmsg;
