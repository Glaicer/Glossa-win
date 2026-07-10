# Changelog

All notable changes to Glossa for Windows will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.4.0] - 2026-07-10

### Added

- Initial Windows port of Glossa, feature-aligned with Linux 1.4.0.
- Native Windows clipboard (`arboard`) and paste (`enigo` / SendInput).
- Global hotkey backend via `global-hotkey` (toggle and push-to-talk).
- System tray with status icons and quick toggles (mic stream, AI enhancer, append space, mode).
- Loopback TCP IPC for `glossa ctl` / `glossa status` (`127.0.0.1:47823` by default).
- Esc-to-cancel while recording.
- Autostart via Startup folder (`glossa service start|stop|restart`).
- `glossa doctor` diagnostics for Windows.
- Shared core: audio capture, silence trim, STT clients (Groq/OpenAI/compatible), optional LLM enhancer, pure state machine.
