# Modified vendored crates

The crates in this folder are copies of crates.io releases, licensed under the Apache License 2.0 (each crate's `LICENSE-APACHE`). The files below were changed for Jot. The registry extraction files (`.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock`, `Cargo.toml.orig`) were also removed.

## gpui-base 0.7.3

- `src/input/base/state.rs`: wheel scrolling by WinUI's notch size (using the system's lines and characters per notch), a hidden built-in scrollbar, a scroll extent query, re-binding an editor to a new window when its tab moves between windows, an undo availability query, atomic multi-range replace, and line-ending normalization on paste.
- `src/input/base/undo_manager.rs`: undo availability is exposed outside tests.
- `src/input/editor/display_map/text_wrapper.rs`: lines made only of characters from certain Unicode ranges wrap by character width without text shaping.

## gpui-pre-windows 0.3.9

- `src/directx_renderer.rs`: alpha blending composites correctly over transparent windows.
- `src/events.rs`, `src/util.rs`, `src/window.rs`, `src/platform.rs`: an app-wide light or dark appearance override (`set_window_appearance`).
- `src/platform.rs`, `src/gpui_windows.rs`: a save dialog with a title, file types, a default extension and a choice drop-down (`prompt_for_save_path`).
- `src/window.rs`, `src/events.rs`, `src/gpui_windows.rs`: a minimum window size that can be changed after the window is created (`set_window_min_size`).
