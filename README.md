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

## Configuration

Configuration is loaded from the platform configuration directory as
`papr/config.toml`. Missing configuration is created automatically the first
time `papr` runs.

Run this command to print the exact paths used on your machine:

```sh
cargo run --release --bin papr -- paths
```

The output includes:

- `config`: the TOML configuration file to edit.
- `database`: the SQLite database file.
- `downloads`: the default PDF download directory.

On Linux, the config file is usually:

```text
~/.config/papr/config.toml
```

On Linux, the default PDF download directory is usually:

```text
~/.local/share/papr/papers
```

To change where downloaded PDFs are stored, edit `config.toml` and set
`download_path`:

```toml
download_path = "/home/you/Documents/papers"
```

To scan existing PDF folders as part of the library, add them to
`library_folders`:

```toml
library_folders = [
  "/home/you/Documents/papers",
  "/home/you/Downloads/research",
]
```

An example complete config:

```toml
theme = "catppuccin"
startup_page = "dashboard"
pdf_viewer = "xdg-open"
library_folders = [
  "/home/you/Documents/papers",
]
download_path = "/home/you/Documents/papers"
mouse = false
```

After updating `library_folders`, run the index command to import PDFs without
opening the TUI:

```sh
cargo run --release --bin papr -- index
```

When `download_path` is unset, downloaded PDFs are saved to the default
`downloads` path printed by `papr paths`. The download directory is also watched
and indexed automatically while the TUI is running.
