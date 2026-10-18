# Jot

A notepad app without the clutter. Written in Rust with GPUI and GPUI Kit.

Jot looks at home on Windows 11 and works the way you'd expect: open it, type, save.

## Features

- **Tabs:** smooth animations; drag to reorder, or drag a tab out into its own window
- **Editing:** find and replace that stays quick on large files, plus go to line, word wrap, status bar and zoom
- **Files:** drag text files onto the window to open them
- **Themes:** light and dark, following Windows, with a one-click toggle
- **Encodings:** UTF-8, plus legacy encodings, saved back the way they came
- **Keyboard:** full access to the menus (Alt, F10, Shift+F10)
- **Plain text only, by design:** no formatting, no markdown

## Stateless

Jot saves nothing between sessions: no session restore, no cache. It collects no data and makes no network connections. It's a single portable `Jot.exe` with no installer.

## Download

Get `Jot.exe` from the [latest release](https://github.com/therealmarrakesh/jot/releases/latest) and run it, or build it from source with Cargo. Requires Windows 11 (64-bit).

Jot isn't code-signed yet, so SmartScreen may warn you the first time. Click **More info → Run anyway**.

## Third-party licenses

Run `Jot.exe --notices`.
