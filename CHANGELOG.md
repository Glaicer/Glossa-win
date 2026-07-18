# Changelog

All notable changes to Glossa for Windows will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-07-18

First release of Glossa for Windows — a headless speech-to-text daemon for Windows 10/11.

### Added

- Background daemon with system tray status icons (idle / recording / processing) and light/dark themes.
- Global hotkey input (`input.backend = "hotkey"`) in toggle or push-to-talk mode (default `Ctrl+Alt+Space`).
- Interactive hotkey picker from setup and the tray (“Change shortcut…”).
- First-run setup wizard (`glossa setup`, also on first launch when no config exists): installs binary and assets, writes config, stores the STT API key as a user environment variable, optional start-at-login, optional start daemon now.
- Full Settings dialog from the tray (input, provider, paste, theme, LLM enhancer, and related options).
- Native Windows clipboard and paste (`Ctrl+V`, `Ctrl+Shift+V`, or `Shift+Insert`); restores previous clipboard contents after pasting.
- STT providers: Groq, OpenAI, and other OpenAI-compatible endpoints (including self-hosted).
- Optional OpenAI-compatible LLM enhancer for punctuation and transcription cleanup (API key via env, not written into config).
- Esc cancels an in-progress recording.
- Loopback TCP IPC for CLI control (`glossa ctl`, `glossa status`; default `127.0.0.1:47823`).
- Autostart via Startup folder (`glossa service start|stop|restart`).
- `glossa doctor` diagnostics for Windows.
- GitHub-release self-updater (`glossa update` and tray **Update…**).
- Shared core: CPAL capture, silence trim, WAV pipeline, pure state machine (`AppActor`).

[1.0.0]: https://github.com/Glaicer/Glossa-win/releases/tag/v1.0.0
