# Jot

A plain-text editor for Windows 11.

Jot keeps the familiar look, tabs, and keyboard shortcuts of Windows 11 Notepad while stopping strictly at plain text: no formatting, no Markdown, no AI features, and nothing saved between sessions. It opens large files quickly, supports dragging tabs to reorder or break them out into new windows, follows your Windows light or dark theme (including High Contrast), and ships as a single portable `Jot.exe` with no installer.

## Download

Get `Jot.exe` from the [latest release](https://github.com/therealmarrakesh/jot/releases/latest). Requires 64-bit Windows 11 (22H2+ for the Mica Alt backdrop).

`Jot.exe` is not code-signed, so Windows SmartScreen may prompt on first run (**More info -> Run anyway**).

---

## Design

Jot is written in Rust directly against Win32, Direct2D 1.1, DirectWrite, and DirectComposition, with no GUI framework and no WebView. It uses a small in-house flexbox layout engine and flat scene arena with typed hit targets and top-down event dispatch, keeping the standalone release binary around 1.5 MB with only three dependency families (`ropey`, `unicode-segmentation`, and the `windows` crates).

- **Text & I/O:** Rope-backed editing (`ropey`) with chunked streaming I/O, atomic saves with fallback for locked/read-only-directory targets, and round-trip preservation of UTF-8 (with or without BOM), UTF-16 LE/BE, Windows ANSI encodings, and line endings (`CRLF`, `LF`, `CR`).
- **Search & editing:** Incremental soft-wrap with hanging indent and 8-column tab stops, grapheme-cluster navigation, coalescing undo/redo with bulk snapshots, and chunked find/replace with per-character case-insensitive matching.
- **Windows 11 integration:** Custom borderless titlebar with Snap Layouts, Mica Alt backdrop, High Contrast palette and system caret support, Per-Monitor V2 DPI, IME composition, OLE file drop target, native file dialogs with encoding selection, Alt/F10 KeyTips, and SID/session-scoped single-instance named-pipe handoff.

### Code layout

- `src/platform/`: Win32 windowing, borderless chrome, DPI/IME/OLE file drop, file dialogs, clipboard, named pipe, and typed background jobs/timers.
- `src/gfx/`: Direct3D 11 / DirectComposition / Direct2D 1.1 renderer and DirectWrite shaping and line-break analysis.
- `src/ui/`: Flex layout solver, flat node/hit scene graph, animations, and Fluent controls.
- `src/editor/`: Rope buffer, undo/redo history, soft-wrap index, and editor viewport.
- `src/app.rs`, `src/window.rs`: Multi-window lifecycle, focus/shortcut routing, shared primitives (`theme`, `color`, `geom`, `types`, `command`, `instance`), and feature modules (`tabs`, `tab_drag`, `menu`, `keytips`, `find`, `dialog`, `tooltip`, `scrollbar`, `files`, `document`).
- `build.rs`: Embeds `assets/jot.ico`, the Windows 11 manifest (`PerMonitorV2`, `SegmentHeap`, Common Controls v6), and version info via `rc.exe`, and generates third-party notices in `OUT_DIR` (embedded via `include_str!`).

## Building

Requires a Rust MSVC toolchain (edition 2024, Rust 1.91+) and the Windows 10/11 SDK (`build.rs` locates `rc.exe` automatically).

```powershell
cargo build --release
cargo test
```

The release binary is written to `target\release\Jot.exe` (with line-table symbols in `target\release\Jot.pdb`). `cargo test` runs the unit suite along with automated Win32/Direct2D GUI regression tests (requires an interactive Windows desktop session with D3D11/WARP).

## Third-party notices

All runtime dependencies are MIT-licensed. Run `Jot.exe --notices` to view the bundled license text.
