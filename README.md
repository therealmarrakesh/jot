# Jot

A plain-text editor for Windows 11.

Jot keeps the familiar look, tabs, and keyboard shortcuts of Windows 11 Notepad while stopping strictly at plain text: no formatting, no Markdown, no AI features, and nothing saved between sessions. It opens large files quickly, supports dragging tabs to reorder or break them out into new windows, follows your Windows light or dark theme (including High Contrast), and ships as a single portable `Jot.exe` with no installer.

## Download

Get `Jot.exe` from the [latest release](https://github.com/therealmarrakesh/jot/releases/latest). Runs on Windows 10 and Windows 11 (Windows 11 22H2+ for the Mica Alt backdrop).

`Jot.exe` is not code-signed, so Windows SmartScreen may prompt on first run (**More info -> Run anyway**).

---

## Design

Jot is written in Rust directly against Win32, Direct2D 1.1, DirectWrite, and DirectComposition, with no GUI framework and no WebView. It uses a small in-house flexbox layout engine and generic flat scene arena with typed hit targets and explicit modal event routing, keeping the standalone release binary around 1.5 MB with only three dependency families (`ropey`, `unicode-segmentation`, and the `windows` crates).

- **Text & I/O:** Rope-backed editing (`ropey`) with chunked streaming I/O, atomic saves with fallback for locked/read-only-directory targets, and round-trip preservation of UTF-8 (with or without BOM), UTF-16 LE/BE, Windows ANSI encodings, and line endings (`CRLF`, `LF`, `CR`).
- **Search & editing:** Headless text model with transaction undo/redo history and non-polluting IME preedit, prefix-sum incremental soft-wrap with hanging indent and 8-column tab stops, grapheme-cluster navigation, and chunked find/replace with per-character case-insensitive matching.
- **Windows 11 integration:** Custom borderless titlebar with Snap Layouts, Mica Alt backdrop, High Contrast palette and system caret support, Per-Monitor V2 DPI, IME composition, `WM_DROPFILES` drag-and-drop, native file dialogs with encoding selection, Alt/F10 KeyTips, and single-instance mutex + `WM_COPYDATA` handoff.

### Code layout

- `src/platform/`: Win32 event loop, main/overlay windows, borderless chrome, DPI/IME/`WM_DROPFILES`, native file dialogs, clipboard, system settings, single-instance `WM_COPYDATA` handoff, and UI-thread timer/job runner.
- `src/gfx/`: Direct3D 11 / DirectComposition / Direct2D 1.1 `Gpu` & `Surface` renderer and DirectWrite `TextStyle`/`Label`/`ShapedRow` shaping and line-break analysis.
- `src/ui/`: Flex layout solver with same-frame anchors and fit-content measurement, generic flat scene graph, key-based animation tracks, unified `OverlaySurface`, and Fluent controls.
- `src/editor/`: Headless `TextModel`, transaction `History`, rope buffer & selection, prefix-sum soft-wrap index, and prepare/paint editor view.
- `src/window/`, `src/app.rs`: Multi-window `Shell`/`Cx`/`Effect` lifecycle, `TabSet` invariants, explicit `InputMode` routing, and window chrome.
- Shared primitives: `ids`, `diag`, `theme`, `color`, `geom`, `types`, and `command`.
- Feature modules: `src/menu/` (`model`, `popup`, `bar`, `render`), `src/find/` (`matcher`, `bar`), `tabs`, `tab_drag`, `keytips`, `dialog`, `tooltip`, `scrollbar`, `files` (`FileOps` workflow & background I/O executor), and `document` (streaming codec & atomic save).
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
