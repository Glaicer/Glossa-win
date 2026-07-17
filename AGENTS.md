# Glossa (Windows) — AI Agent Context

---

## Coding guidelines

Same principles as the Linux project: think before coding, simplicity first, surgical changes, goal-driven execution.

---

## What Is Glossa

Glossa is a **headless speech-to-text daemon** for **Windows**. It captures microphone audio on a global hotkey, transcribes it via a cloud API (Groq / OpenAI), places the text into the Windows clipboard, and pastes it into the active window with a native keyboard shortcut.

**Target platform:** Windows 10/11 (x86_64).

This repository is a Windows port of the Linux project at `../Glossa`. Shared crates (`glossa-core`, `glossa-app`, `glossa-audio`, `glossa-stt`) keep the same architecture; platform adapters live in `glossa-platform-windows`.

---

## Project Layout

```
glossa-win/
├── Cargo.toml
├── crates/
│   ├── glossa-core/              # pure types, config, state
│   ├── glossa-app/               # state machine, ports, actor
│   ├── glossa-audio/             # CPAL capture, WAV, cues
│   ├── glossa-stt/               # Groq/OpenAI/compatible HTTP clients
│   ├── glossa-platform-windows/  # clipboard, paste, hotkeys, tray, IPC, doctor
│   └── glossa-bin/               # CLI + composition root
└── contrib/
    ├── assets/                   # tray icons + cue sounds
    └── examples/config.toml
```

### Windows platform mapping

| Linux | Windows |
|-------|---------|
| XDG Desktop Portal GlobalShortcuts | `global-hotkey` (`RegisterHotKey`) |
| `wl-copy` / `wl-paste` | Native clipboard (`arboard`) |
| `dotool` / `dotoolc` | Native `SendInput` via `enigo` |
| Unix socket IPC | Loopback TCP (`127.0.0.1:47823`) |
| `$XDG_RUNTIME_DIR/glossa` | `%LOCALAPPDATA%\Temp\glossa` |
| systemd user service | Startup folder shortcut / Task Scheduler |
| GTK tray + dialogs | `tray-icon` + Win32 `MessageBox` |

---

## Architecture Rules

Unchanged from Linux:

- One state machine (`AppActor`), pure reducer, port traits
- Command sources: global hotkey + CLI IPC + tray menu
- State flow: Idle → Recording → Processing → Pasting → Idle
- Esc cancels an in-progress recording

---

## Build & Run (on Windows)

```powershell
cargo build --release
cargo test --workspace

# Run daemon
$env:GROQ_API_KEY = "..."
.\target\release\glossa.exe --config $env:APPDATA\glossa\config.toml daemon

# Control
.\target\release\glossa.exe ctl toggle
.\target\release\glossa.exe status
.\target\release\glossa.exe doctor
```

Cross-compiling from Linux requires a Windows target and linker (`x86_64-pc-windows-gnu` or `x86_64-pc-windows-msvc`).

---

## Configuration notes

- `input.backend = "hotkey"` (alias of `"portal"`) enables the global hotkey backend
- `input.shortcut` accepts `Ctrl+Alt+Space` or `<Ctrl><Alt>space`
- `control.socket_path = "auto"` → `127.0.0.1:47823`
- `paste.clipboard_command` / `paste.type_command` are kept for schema compatibility; the Windows build always uses native adapters
- Default config path: `%APPDATA%\glossa\config.toml`
- Default asset path: `%LOCALAPPDATA%\glossa\assets\...`

---

## Agent skills

### Issue tracker

Issues live as markdown under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default role strings: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout (`CONTEXT.md` + `docs/adr/` at repo root). See `docs/agents/domain.md`.
