# papr

**Papr** is a fast terminal-based research paper explorer in Rust. Search, discover, and download papers directly from arXiv. Organize your library, manage reading queues, take notes, and track stats to keep your research organized all without leaving your terminal.

By merging online arXiv discovery with local file organization, Papr provides a unified, keyboard-driven environment designed for focused academic research. The application is built using Ratatui, Tokio, Reqwest, and SQLite, delivering a lightweight, offline-capable utility that operates entirely under your control.

---

## Why Rust?

Papr is implemented in Rust to meet the performance, reliability, and security demands of a modern academic workflow:
* **Performance:** Instant application startup and rapid indexing of thousands of local PDFs.
* **Asynchronous Concurrency:** Leverages Tokio's runtime to run background downloads, watch filesystem changes, and perform network requests concurrently without freezing the user interface.
* **Safety and Stability:** Prevents common runtime issues such as memory corruption, race conditions, and application crashes.
* **Local Persistence:** Uses SQLite with Write-Ahead Logging (WAL) enabled, providing fast, ACID-compliant database operations.
* **Zero-Runtime Overhead:** Compiles into a single, self-contained binary with low CPU and memory footprints.

---

## Features

* **arXiv Explorer:** Search the arXiv repository by title, author, category, abstract, or DOI. View full metadata, category listings, and journal references directly.
* **Automatic Title Sanitization:** Downloads PDFs in the background and automatically names them using their sanitized, cross-platform paper titles rather than legacy arXiv identifiers.
* **Duplicate Merging:** Automatically resolves SQLite unique constraint conflicts (`arxiv_id`, `pdf_path`, `doi`) by merging duplicate entries (preserves notes, bookmarks, and progress) during downloads and scans.
* **Consistent Workspace Actions:** Manage notes (`n`), bookmarks (`B`), collections (`s`), and file renames (`R`) uniformly across the Library, Collections, Bookmarks, and Downloads tabs.
* **Accurate Storage Statistics:** Monitor reading statistics, active streaks, and disk usage. Folders are canonicalized to prevent duplicate PDF counts, even if your downloads folder is nested within a library path.
* **Filesystem-Synced Collections:** Move papers into collections inside the TUI to automatically rename and migrate files on your disk.
* **Markdown Annotation:** Take dedicated, auto-saved notes per paper. Features a styled Markdown live preview accessible with the `Tab` key.
* **Process-Isolated Plugins:** Extend the application through process-isolated plugins using a versioned JSON RPC protocol.

---

## Requirements

* A terminal with color and alternate-screen support.
* Stable Rust toolchain (when building from source).
* A system PDF viewer available through the platform default:
  * Linux: `xdg-open`
  * macOS: `open`
  * Windows: `cmd /C start`

*Note: The default PDF viewer command can be overridden in the configuration.*

---

## Installation

### Build and Install from Source

```sh
git clone https://github.com/AfrozSaqlain/Papr.git
cd Papr
cargo install --path crates/papr
```

Ensure Cargo's binary directory is added to your shell `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

### Run Without Installing

```sh
cargo run --release --bin papr
```

---

## Configuration

Papr generates a default configuration file (`config.toml`) automatically upon its first run. To inspect your resolved paths (configuration, database, downloads, and plugins), execute:

```sh
papr paths
```

### Path Locations

| OS | Configuration File | SQLite Database & Assets |
| --- | --- | --- |
| **Linux** | `~/.config/papr/config.toml` | `~/.local/share/papr/` |
| **macOS** | `~/Library/Application Support/papr/config.toml` | `~/Library/Application Support/papr/` |
| **Windows** | `%APPDATA%\papr\config.toml` | `%APPDATA%\papr\` |

### Configuration Example (`config.toml`)

Below is a complete configuration structure:

```toml
theme = "catppuccin"
startup_page = "dashboard"
pdf_viewer = "zathura"  # Custom PDF viewer command

# Paths to recursively scan for local PDFs
library_folders = [
  "/home/user/Documents/papers",
  "/home/user/Downloads/research",
]

# Destination directory for new downloads
download_path = "/home/user/Documents/papers"

# Comma-separated interests to populate the Dashboard arXiv feed
dashboard_keywords = "machine learning, gravitational waves, astrophysics"

mouse = false
enabled_plugins = []
```

#### Custom Viewer Arguments
If you need custom arguments for your PDF viewer, use the `{path}` placeholder:
```toml
pdf_viewer = "zathura --fork {path}"
```
If `{path}` is omitted, the PDF file path is automatically appended as the final argument.

---

## Quick Start

1. Start the application with `papr`.
2. Press `/` to focus the search bar.
3. Enter your query (e.g. `author: Einstein`) and press `Enter`.
4. Use `j`/`k` to select a result and press `Enter` to open the detail view.
5. Press `d` to download the paper.
6. Open the **Downloads** tab. Once completed, press `Enter` to open the PDF.
7. Use `B` to toggle bookmarks, `n` to edit notes, or `s` to categorize it into a collection.

---

## Interface Workspaces

* **Dashboard:** Displays reading statistics, streaks, storage usage, and a daily feed of new arXiv papers matching your keywords.
* **Discover:** Search arXiv, explore paper metadata, and queue background downloads.
* **Library:** View all indexed local PDFs within your configured folders.
* **Reading Queue:** A prioritized list of papers you plan to read next.
* **Collections:** Paper groups mapping directly to subdirectories in your library.
* **Bookmarks:** Quick-access list of bookmarked local PDFs.
* **Authors:** Browse local papers grouped by author name.
* **Notes:** Search and browse all your paper-linked Markdown notes.
* **Downloads:** Active, completed, and failed downloads. Supports `B`, `n`, `s`, and `R` actions on completed downloads.
* **History:** A chronological log of searches, downloads, and paper opens.
* **Statistics:** Detailed reading streaks, totals, top dimensions, and a 12-week reading heatmap.
* **Settings:** Quick summary of active paths, configurations, and plugins.
* **Help:** Interactive keyboard reference map.

---

## Keyboard Reference

### Global Navigation

| Key | Action |
| --- | --- |
| `j` / `Down` | Move down / select next item |
| `k` / `Up` | Move up / select previous item |
| `Enter` / `l` / `Right` | Open the selected item, section, or paper |
| `Left` | Return focus to the sidebar navigation |
| `h` | Go back to the previous screen or list |
| `/` | Start a new arXiv search |
| `Ctrl+P` | Open the command palette |
| `?` | Toggle the help helper screen |
| `q` | Close active popups, or exit the application |

### Discovery & Details

| Key | Action |
| --- | --- |
| `Enter` | Open the detail page for the selected search result |
| `j` / `k` | Scroll details view |
| `d` | Download PDF |
| `o` | Open the paper webpage in your default browser |
| `n` | Open/edit the Markdown note |
| `s` | Assign paper to a collection |
| `B` | Toggle bookmark |
| `r` | Refresh/retry the current search |
| `h` / `Esc` | Return to results |

### Library & Collections

| Key | Action |
| --- | --- |
| `Enter` / `p` | Open the PDF in your default viewer |
| `r` | Scan library folders for new files |
| `n` | Edit Markdown note |
| `s` | Move PDF file to a collection folder |
| `B` | Toggle bookmark |
| `R` | Rename a PDF file or collection folder |
| `c` / `n` | Create a new collection and folder |

### Downloads Tab

| Key | Action |
| --- | --- |
| `Enter` / `l` | Open the downloaded PDF (once completed) |
| `B` | Toggle bookmark on the downloaded paper |
| `n` | Edit Markdown note for the downloaded paper |
| `s` | Move the downloaded paper to a collection |
| `R` | Rename the downloaded PDF |

### Markdown Editor

| Key | Action |
| --- | --- |
| *Type* | Add text |
| `Enter` | Insert new line |
| `Backspace` | Delete character |
| `Tab` | Toggle between editor and styled preview |
| `Esc` | Save note and exit editor |

---

## Common Workflows

### Finding and Downloading
Press `/` to start searching. You can type keywords or use search field prefixes to narrow down results:
```text
author: Saqlain Afroz
title: gravitational wave inference
abstract: neural networks
category: gr-qc
```
Open a paper details page and press `d`. Papr automatically downloads the PDF, names it using its sanitized paper title, and saves it. If the paper already exists in your library, Papr automatically merges the records so you don't end up with duplicate metadata.

### Organizing Into Collections
Collections sync directly with folders on your drive. When you select a paper and press `s`, you can choose a collection or type a new name. Papr creates the folder and moves the PDF file there automatically, keeping your physical directories clean.

### Writing Notes
Press `n` on any paper to open the Markdown editor. Jot down summaries, math, or ideas. Tap `Tab` to preview your formatting, and hit `Esc` to save it straight to the SQLite database.

---

## Project Structure

Papr is split into two core crates:
* **`papr-core`:** Handles database migrations, SQLite queries, configuration loading, downloading, indexing local directories, and plugins.
* **`papr`:** Handles the terminal UI loop (Ratatui + Crossterm), input handling, async orchestration, and launching external PDF viewers.

---

## CLI Utilities

You can manage Papr and run headless tasks right from the terminal:

```sh
papr                         # Start the TUI
papr paths                   # Print where configs, database, and folders are
papr index                   # Scan library folders and index new files (headless)
papr completions <SHELL>     # Generate completions (bash, zsh, fish)
papr plugins                 # Check discovered plugins and manifests
papr plugin <ID> <EVENT>     # Run plugin events manually to test them
```

---

## Contributing

Contributions are welcome! If you would like to help improve Papr, keep these guidelines in mind:

1. Keep modifications scoped to the owning crate.
2. Avoid `unwrap`, `expect`, deliberate panics, or `unsafe` code in production paths.
3. Database updates require append-only migrations and a corresponding test.
4. Ratatui render functions must remain strictly pure (no external file or database reads/writes during rendering).

Before submitting a pull request, verify all checks pass:
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

---

## License

Papr is open-source software distributed under the [MIT License](LICENSE).
