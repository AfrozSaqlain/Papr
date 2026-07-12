# papr

**papr** is a keyboard-first terminal workspace for discovering, collecting,
reading, and organizing academic papers. It combines arXiv discovery, a local
PDF library, Markdown notes, collections, bookmarks, downloads, reading
history, and research statistics in one responsive TUI.

The application is written in Rust using Ratatui, Tokio, Reqwest, and SQLite.
It does not include AI, chatbot, summarization, recommendation, or embedding
features.

## Features

- Search arXiv and inspect paper metadata, abstracts, authors, categories, DOI,
  journal references, and PDF resources.
- Download PDFs in the background with progress tracking and atomic completion.
- Import and monitor multiple local PDF directories.
- Detect duplicate PDFs using SHA-256 content hashes.
- Open local PDFs with the operating system viewer or a configured command.
- Maintain one autosaving Markdown note per paper with a styled preview.
- Organize papers with collections and bookmarks.
- Track paper and PDF activity in a persistent reading history.
- View reading streaks, monthly and yearly totals, storage usage, and an
  activity heatmap.
- Customize the interface with built-in or user-defined themes.
- Extend the application through a versioned, process-isolated plugin API.

## Requirements

- A terminal with color and alternate-screen support.
- Stable Rust when building from source.
- A PDF viewer available through the platform default:
  - Linux: `xdg-open`
  - macOS: `open`
  - Windows: `cmd /C start`

The PDF viewer command can be overridden in the configuration.

## Installation

### Build and install from this repository

```sh
git clone https://github.com/AfrozSaqlain/Papr.git
cd Papr
cargo install --path crates/papr
```

Ensure Cargo's binary directory is on `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

### Run without installing

```sh
cargo run --release --bin papr
```

## Quick Start

1. Start the application with `papr`.
2. Press `/` to search arXiv.
3. Enter a title, author, keyword, category, or DOI and press `Enter`.
4. Use `j` and `k` to select a result, then press `Enter` to open its paper
   page.
5. Press `d` to download the paper.
6. Open **Library** from the sidebar and press `Enter` or `p` on a downloaded
   PDF to launch the configured viewer.
7. Press `n`, `s`, or `B` on a paper to add notes, collections, or a bookmark.

## Interface

| Workspace | Purpose |
| --- | --- |
| Dashboard | Latest arXiv papers, library counters, activity, streak, and storage |
| Discover | arXiv search and paper details |
| Library | Imported and downloaded local papers |
| Reading Queue | Reserved reading-priority workspace |
| Collections | Selectable paper groups with drill-down browsing |
| Bookmarks | Bookmarked papers and positions |
| Authors | Reserved author workspace |
| Notes | Entry point for paper-linked Markdown notes |
| Downloads | Active, completed, and failed PDF transfers |
| History | Chronological research and reading activity |
| Statistics | Reading totals, streak, top dimensions, and heatmap |
| Settings | Configuration summary and discovered plugins |
| Help | Keyboard reference |

## Keyboard Reference

### Global navigation

| Key | Action |
| --- | --- |
| `j` / `Down` | Move down |
| `k` / `Up` | Move up |
| `Enter` / `l` / `Right` | Open the selected item or section |
| `h` / `Left` | Return to the dashboard or previous view |
| `/` | Open arXiv search |
| `Ctrl+P` | Open the command palette |
| `?` | Toggle keyboard help |
| `q` | Quit or close the active help/detail view |

### Discovery and paper pages

| Key | Action |
| --- | --- |
| `Enter` | Open the selected paper page |
| `j` / `k` | Scroll paper details |
| `d` | Download the PDF |
| `n` | Open the paper's Markdown note |
| `s` | Add the paper to a collection |
| `B` | Toggle the paper bookmark |
| `r` | Repeat the current arXiv search |
| `h` / `Esc` | Return to search results |

### Local library

| Key | Action |
| --- | --- |
| `Enter` / `p` | Open the selected local PDF |
| `r` | Rescan configured library folders |
| `n` | Edit the paper note |
| `s` | Add to a collection |
| `B` | Toggle bookmark |

### Markdown notes

| Key | Action |
| --- | --- |
| Normal text input | Edit the Markdown source with autosave |
| `Enter` | Insert a new line |
| `Backspace` | Delete the previous character |
| `Tab` | Toggle source and styled preview |
| `Esc` | Save and return to the originating paper |

## Discovering Papers

Press `/` from anywhere to focus the arXiv search field. Searches run
asynchronously, so terminal input and rendering remain responsive.

Results show the title, publication date, authors, and primary category. Paper
pages include the title, authors, dates, categories, abstract, DOI, journal
reference, and arXiv/PDF resource URLs when available.

Network failures are shown in Discover. Press `r` to retry the current query.

## Library and PDF Management

papr scans every directory in `library_folders` recursively. The download
directory is always monitored while the TUI is running.

For each PDF, papr records its inferred title, path, file size, SHA-256 content
hash, indexing timestamp, and linked research metadata. Duplicate content is
ignored even when it appears under another filename. Replacing a file at an
existing path refreshes its hash and size.

Downloads stream into a temporary `.part` file. The file is renamed to its
final `.pdf` path only after the transfer completes successfully.

### Import PDFs without opening the TUI

```sh
papr index
```

This scans configured library folders and reports how many PDFs were found and
newly imported.

## Notes and Organization

Every paper can have one Markdown note. Notes are stored in SQLite and saved as
you type. Preview mode highlights headings, lists, quotations, and code blocks.

Collections are case-insensitive and idempotent, and a paper can belong to
multiple collections. Use uppercase `B` to toggle a whole-paper bookmark.
Lowercase `b` remains reserved for citation functionality.

Open **Collections**, select a collection with `j`/`k`, and press `Enter` to
browse its papers. The paper list is also keyboard navigable. Press `Enter` or
`p` to open a paper's local PDF, and press `h` or `Esc` to return to the
collection list. Papers without a downloaded PDF remain visible and are
labeled as metadata-only.

Libraries created by older papr versions are upgraded automatically: legacy
tags and their paper assignments are copied into same-named collections.

## Dashboard, History, and Statistics

The dashboard combines a non-blocking latest-paper feed with local library,
queue, download, unread, streak, activity, collection, and storage data.

Opening remote paper details or local PDFs records reading history. Searches,
downloads, note opens, bookmarks, and collection changes are also
recorded as research activity.

Statistics include the current streak, monthly and yearly reading totals,
session count, average reading time, most active weekday, most-read author and
journal, and a normalized 12-week activity heatmap.

## Configuration

papr creates `config.toml` automatically on first launch. Print all resolved
paths with:

```sh
papr paths
```

This reports the configuration file, SQLite database, default download
directory, and plugin directory. Typical Linux locations are:

```text
~/.config/papr/config.toml
~/.local/share/papr/papr.db
~/.local/share/papr/papers
~/.local/share/papr/plugins
```

macOS and Windows use their standard platform application directories.

### Complete configuration example

```toml
theme = "catppuccin"
startup_page = "dashboard"
pdf_viewer = "xdg-open"

library_folders = [
  "/home/you/Documents/papers",
  "/home/you/Downloads/research",
]

download_path = "/home/you/Documents/papers"
mouse = false
enabled_plugins = []
```

### Themes

Built-in themes are `catppuccin`, `tokyo-night`, `gruvbox`, `nord`,
`dracula`, and `light`. The `theme` setting may also point to a custom theme
TOML file.

### PDF viewer

Use a command name:

```toml
pdf_viewer = "zathura"
```

Commands with arguments and a `{path}` placeholder are supported:

```toml
pdf_viewer = "my-viewer --new-window {path}"
```

If `{path}` is omitted, the PDF path is appended as the final argument.

## Command-Line Utilities

```text
papr
papr paths
papr index
papr completions <SHELL>
papr plugins
papr plugin <ID> <EVENT> [--timeout <SECONDS>]
```

### Shell completions

```sh
papr completions bash > papr.bash
papr completions zsh > _papr
papr completions fish > papr.fish
```

Install the generated file using the conventions of your shell.

## Plugins

Plugins are external processes using a versioned JSON protocol. They are
disabled by default and must be explicitly enabled:

1. Place the plugin bundle in the directory printed by `papr paths`.
2. Run `papr plugins` to inspect discovery and validation results.
3. Add its ID to `enabled_plugins` in `config.toml`.
4. Exercise an event with `papr plugin <ID> <EVENT>` when troubleshooting.

See [Plugin Protocol](docs/PLUGINS.md) for manifests, capabilities, JSON
requests, response actions, timeouts, and limits.

## Troubleshooting

### A PDF does not open

1. Run `papr paths` and confirm the library/download locations.
2. Verify the file still exists at the path shown in Library.
3. Test the configured viewer directly from the terminal.
4. Set an explicit `pdf_viewer` command in `config.toml`.

Viewer launch failures are reported in the status line without terminating the
application.

### Search fails

- Confirm internet access to `https://export.arxiv.org`.
- Press `r` in Discover to retry.
- arXiv may temporarily throttle requests or return service errors.

The local library, notes, collections, history, and statistics remain available
without network access.

### Imported PDFs do not appear

- Confirm their directories are listed in `library_folders`.
- Run `papr index` and inspect the reported counts.
- Confirm the files use a `.pdf` extension.
- Duplicate file content is intentionally imported only once.

### Plugin is disabled or invalid

Run `papr plugins`. The output includes enabled state and manifest validation
diagnostics. Execution also requires the exact plugin ID in `enabled_plugins`.

## Development

See [Architecture](docs/ARCHITECTURE.md) for module ownership and extension
boundaries, and [Contributing](CONTRIBUTING.md) for contribution requirements.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo package --workspace --allow-dirty --no-verify
```

The project forbids unsafe code and denies `unwrap`, `expect`, and deliberate
panic usage through workspace lint configuration.

## License

papr is available under the [MIT License](LICENSE).
