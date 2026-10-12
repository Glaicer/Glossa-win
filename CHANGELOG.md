# Changelog

All notable changes to Glossa for Windows will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- On-screen recording/processing indicator (`ui.overlay`, on by default): a dark, click-through pill at the bottom-center of the primary display showing a microphone and a live waveform driven by the input level while recording, and a server icon with pulsing dots while processing. Failures are best-effort and never block dictation.

## [1.1.0] - 2026-07-21

### Added

- Auto-stop recording when `audio.max_duration_sec` elapses (timer armed on start; cleared on manual stop, Esc cancel, or shutdown; stale timers ignored after session changes).
- API key storage via Windows Credential Manager (`cred:NAME`) from setup and Settings.
- On daemon start, legacy keys in `HKCU\Environment` / process env referenced by `env:NAME` are migrated into Credential Manager and cleared from the user environment.

### Changed

- Default `audio.max_duration_sec` raised from `120` (2 min) to `1200` (20 min).
- Default `logging.level` lowered from `info` to `warn`.
- Setup wizard and Settings store STT (and LLM enhancer) API keys in Credential Manager instead of user environment variables; config references them as `cred:NAME`.

### Security

- API keys are no longer persisted in `HKCU\Environment`, where they were visible to other processes as plain environment variables.

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

[1.1.0]: https://github.com/Glaicer/Glossa-win/releases/tag/v1.1.0
[1.0.0]: https://github.com/Glaicer/Glossa-win/releases/tag/v1.0.0
