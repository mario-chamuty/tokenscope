# TokenScope

Desktop dashboard for Claude Code token usage and cost. It reads the session transcripts Claude Code writes to `~/.claude/projects`, keeps its own history in a local SQLite database (Claude Code deletes old transcripts, the database does not), and shows usage by day, project, model and session, plus a small always-on-top mini window and PDF/CSV reports.

Built with Tauri v2 (Rust backend, plain HTML/JS frontend in `src/`).

## Development

Requirements: Rust (stable) and the Tauri CLI (`cargo install tauri-cli --version "^2" --locked`). On Linux also the WebKitGTK build dependencies listed in `.github/workflows/build.yml`.

```
cd src-tauri
cargo test
cargo tauri dev
```

## Installers

`.github/workflows/build.yml` builds the installers on GitHub Actions (manual run or a `v*` tag):

| Platform | Artifacts |
|---|---|
| Windows | NSIS `.exe`, `.msi` |
| Linux | `.deb`, `.AppImage` |
| macOS | `.dmg` for Apple Silicon and Intel |

Locally on Windows: `./build.ps1`.

The installers are not code-signed. Windows SmartScreen and macOS Gatekeeper will warn on first launch.

## Data

- Database and settings: the app data directory for `sk.versiontwo.tokenscope`. Data from 1.4.0 (`com.versiontwo.tokscope`) is copied over on first launch.
- Schema v6 keys turns by Claude's `message.id`. Upgrading from an older version backs up the database next to it (`tokenscope.db.pre-v6.bak`, safe to delete once the numbers look right) and corrects the previously inflated token and cost totals.

## License

Copyright (c) 2026 Version Two s.r.o. (https://www.versiontwo.sk), maintainer Mario Chamuty.
