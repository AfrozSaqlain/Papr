# papr

`papr` is a keyboard-first terminal workspace for academic papers. Milestone 1
provides the application shell, persistent configuration, SQLite storage,
navigation, command palette, help overlay, and a configurable theme engine.

## Run

```sh
cargo run --release --bin papr
```

Use `j`/`k` or the arrow keys to navigate, `Enter` to open a section, `h` to
return to the dashboard, `Ctrl+P` for the command palette, `?` for help, and
`q` to quit. Press `/` anywhere to search arXiv, then browse results with
`j`/`k` and open a full paper page with `Enter`. Run `papr paths` to print the
resolved configuration and data locations.

Run `papr index` to scan configured library folders without opening the TUI.
Downloaded PDFs stream to a temporary `.part` file and are atomically added to
the library after completion.

From a paper, press `n` for its autosaving Markdown note, `t` to assign a tag,
`s` to add it to a collection, or `B` to toggle a bookmark. Inside the note
editor, `Tab` toggles the styled Markdown preview and `Esc` returns.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Configuration is loaded from the platform configuration directory as
`papr/config.toml`. Missing configuration is created automatically.
