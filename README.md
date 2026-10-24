# Carbon Disk Treemap

A lightweight macOS app that scans disk usage, shows it as an interactive treemap built with the [Carbon Design System](https://carbondesignsystem.com/), and helps you reclaim space safely.

## Features

- **Disk scan.** Scan a folder or volume. The full tree stays in Rust. The UI receives only a small, depth-limited view.
- **Interactive treemap.** A canvas treemap using Carbon tokens, with a folder table, breadcrumbs and light and dark themes.
- **Move to Trash with dry-run preview.** Review exactly what will be moved before you confirm. Nothing is hard deleted.
- **Safe cache cleanup.** Finds regenerable caches and logs from a fixed catalog (for example `~/Library/Caches`, `~/Library/Logs`, Xcode DerivedData, the npm cache), shows their sizes, and moves approved items to the Trash.
- **Full Disk Access guidance.** Explains how to grant access when macOS blocks part of a scan.

### Safety model

- Cleanup candidates come from an allowlist, never from heuristics.
- The UI sends item ids, not paths. Rust owns the paths.
- Items go to the Trash only. Guardrails are re-checked at execution time.

See [`disk-treemap-plan.md`](disk-treemap-plan.md) for the full design.

## Stack

- [Tauri v2](https://tauri.app/) with a Rust core
- React 19, TypeScript and Vite
- `@carbon/react` and `@carbon/icons-react`
- macOS first. Platform-specific code sits behind Rust traits so other platforms can be added later.

## Project layout

- `src/` React UI: `components/`, `treemap/`, `state/`, `ipc/`, `styles/`
- `src-tauri/` Rust crate: `core/` (OS-agnostic), `platform/` (traits and `macos/`), `commands/` (Tauri IPC)

## Getting started

Prerequisites: [Node.js](https://nodejs.org/), [pnpm](https://pnpm.io/), the [Rust toolchain](https://www.rust-lang.org/tools/install), and the [Tauri prerequisites for macOS](https://tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm tauri dev      # run the app in development
pnpm tauri build    # produce a production build
```

## Scripts

| Script | Purpose |
|---|---|
| `pnpm dev` | Start the Vite dev server only |
| `pnpm typecheck` | TypeScript check |
| `pnpm lint` | ESLint (no warnings allowed) |
| `pnpm test` | Run Vitest |
| `pnpm format` / `pnpm format:check` | Prettier |
| `pnpm rust:fmt` / `pnpm rust:fmt:check` | rustfmt |
| `pnpm rust:clippy` | Clippy with warnings as errors |
| `pnpm check` | Typecheck, lint, tests, rustfmt check and clippy |

## Recommended IDE setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
