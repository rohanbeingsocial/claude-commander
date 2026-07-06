# Contributing to Claude Commander

Thanks for your interest in improving Claude Commander. This is a small, local-first
Tauri app — contributions of any size are welcome.

## Dev setup

1. Install the [prerequisites](./README.md#prerequisites) (Node 18+, Rust stable, the
   Tauri 2 Windows toolchain, and Claude Code on your `PATH`).
2. Clone and install:
   ```bash
   git clone https://github.com/rohanbeingsocial/claude-commander.git
   cd claude-commander
   npm install
   ```
3. Run the hot-reloading dev build:
   ```bash
   npm run tauri dev
   ```

## Before you open a PR

- Frontend typecheck / build: `npm run build` (runs `tsc --noEmit` + Vite).
- Rust build: `npm run tauri build --no-bundle` (or `cargo build` inside `src-tauri`).
- Keep changes focused; match the surrounding code style.
- Don't commit build output — `node_modules/`, `dist/`, and `src-tauri/target/` are
  already gitignored.

## Reporting bugs

Open an issue with:

- Windows version and app version
- Steps to reproduce
- What you expected vs. what happened
- Relevant logs (the app writes state to
  `%APPDATA%\com.rohan.claudecommander\commander.db`; **do not** paste anything from your
  account config dirs)

## Good first contributions

- Cross-platform support (macOS / Linux — currently Windows-only via ConPTY).
- Usage-estimation accuracy and calibration.
- Task board UX and keyboard shortcuts.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](./LICENSE).
