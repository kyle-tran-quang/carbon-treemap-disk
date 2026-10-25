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

## Install

**Requirements:** a Mac running macOS 10.15 or later.

1. Download the latest `.dmg` file from the [Releases page](https://github.com/kyle-tran-quang/carbon-treemap-disk/releases).
2. Open the `.dmg` and drag **Carbon Disk Treemap** into your **Applications** folder.
3. Open the app from Applications.

### If macOS says the app can't be opened

This app is unsigned, so macOS shows a security warning when you launch it for the first time, such as "Carbon Disk Treemap can't be opened" or "is damaged". This is expected. For now the app is distributed without an Apple signature, so macOS asks you to confirm that you trust it. You only need to do this once, using one of the workarounds below.

**Option 1: Allow it in System Settings**

1. Try to open the app, then close the message.
2. Open **System Settings > Privacy & Security**.
3. Scroll down to the message about Carbon Disk Treemap and click **Open Anyway**.
4. Enter your password if asked, then click **Open**.

On macOS 14 and earlier, you can instead right-click (or Control-click) the app in Applications, choose **Open**, then click **Open** again.

**Option 2: Use Terminal**

If the message says the app is "damaged", open **Terminal** and run this command, then open the app again:

```sh
xattr -cr "/Applications/Carbon Disk Treemap.app"
```

### Allow access to your files

To scan protected folders, the app needs **Full Disk Access**. If a scan skips folders, the app tells you and shows how to turn it on. You can also go to **System Settings > Privacy & Security > Full Disk Access**, turn on **Carbon Disk Treemap**, and reopen the app.

You stay in control of cleanup. The app shows you what it will remove before you confirm, and it moves items to the Trash instead of deleting them, so you can put them back.

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
