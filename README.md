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
* **Built-in PDF Viewer:** View PDFs directly inside the terminal using high-performance Sixel or Kitty graphics protocols, with smooth physics-based scrolling.
* **Process-Isolated Plugins:** Extend the application through process-isolated plugins using a versioned JSON RPC protocol.

---

## Requirements

* A terminal with color and alternate-screen support.
* Stable Rust toolchain (when building from source).
* (Optional) For the built-in PDF viewer: a terminal emulator supporting Sixel or Kitty graphics protocols, and the `pdftoppm` command-line utility installed on your system path.
* A system PDF viewer available through the platform default:
  * Linux: `xdg-open`
  * macOS: `open`
  * Windows: `cmd /C start msedge ""` (Microsoft Edge is pre-installed on Windows; this can be customized to brave, chrome, firefox, etc.)

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

## Running with Docker

This project includes a Dockerfile that builds `papr` inside a lightweight Alpine-based build environment and produces a small runtime image containing only the compiled executable.

### Build the image

From the root of the repository, run:

```bash
docker build \
    --build-arg USERNAME=$(whoami) \
    --build-arg UID=$(id -u) \
    --build-arg GID=$(id -g) \
    -t papr .
```

This will:

* Build `papr` from source inside a Rust/Alpine builder image.
* Create a lightweight Alpine runtime image.
* Create a user inside the container with the same UID and GID as your host user, avoiding file permission issues when mounting directories.

### Start a shell

```bash
docker run --rm -it \
    --hostname qubit \
    -w /home/$(whoami) \
    papr \
    bash
```

Once inside the container, launch `papr` with:

```bash
papr
```

### Persist your data

By default, any files created inside the container are removed when the container exits because `--rm` deletes the container after it stops.

To keep your PDFs, configuration, and other files on your host machine, mount a directory:

```bash
docker run --rm -it \
    --hostname papr \
    -v /path/to/your/data:/home/$(whoami) \
    -w /home/$(whoami) \
    papr \
    bash
```

Replace `/path/to/your/data` with the directory on your host where you want your library and configuration to be stored.

### Notes

* The final runtime image is based on Alpine Linux and does **not** include the Rust toolchain.
* The Rust compiler and build dependencies exist only in the temporary build stage and are not included in the final image.
* The default Docker hostname is the container ID. Passing `--hostname qubit` gives the container a more readable hostname.

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
theme = "catppuccin-mocha"
startup_page = "dashboard"
pdf_viewer = "zathura"  # Custom PDF viewer command (or "internal" for the built-in terminal PDF viewer)

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

To use the fast, built-in terminal-based PDF viewer (which renders PDFs directly in your terminal if it supports Kitty or Sixel graphics protocols), set `pdf_viewer` to `"internal"`:
```toml
pdf_viewer = "internal"
```

On Windows, the default PDF viewer is configured as `'cmd /C start msedge ""'`, which opens PDFs in Microsoft Edge (pre-installed by default). You can customize this command to use other browsers or PDF viewers (e.g., `'cmd /C start chrome ""'`, `'cmd /C start firefox ""'`, or pointing directly to an executable like `'C:\Program Files\SumatraPDF\SumatraPDF.exe'`).

### Themes

Papr supports both built-in themes and custom user-defined themes via TOML configuration.

#### Built-in Themes

The following theme presets are compiled directly into the application binary:

* `catppuccin-mocha` (default; alias `catppuccin` for backward compatibility)
* `catppuccin-macchiato`
* `catppuccin-frappe`
* `catppuccin-latte`
* `tokyo-night` (or `tokyonight`)
* `gruvbox`
* `nord`
* `dracula`
* `light`
* `rose-pink-dark` (or `rose-pine-dark` / `rose-pink` for backward compatibility)
* `rose-pink-light` (or `rose-pine-light`)
* `everforest`
* `kanagawa`
* `one-dark` (or `onedark`)
* `cyberpunk`

To use a built-in theme, set the `theme` option in your `config.toml` to the corresponding name:
```toml
theme = "gruvbox"
```

#### Custom Themes

You can define your own theme or adapt a color scheme by creating a custom TOML file. Save it anywhere (for example, at `~/.config/papr/my-theme.toml`) and define the following hex color keys:

```toml
name = "My Custom Theme"
background = "#1e1e2e"
surface = "#313244"
text = "#cdd6f4"
muted = "#7f849c"
accent = "#89b4fa"
secondary = "#cba6f7"
success = "#a6e3a1"
warning = "#f9e2af"
error = "#f38ba8"
border = "#45475a"
```

To apply it, configure the `theme` option in your `config.toml` with the absolute path to your theme file:
```toml
theme = "/home/user/.config/papr/my-theme.toml"
```

You can also edit your configuration file directly from Papr's settings tab using built-in editor. The saved changes will take effect immediately.

## Plugins

Papr supports **versioned, process-isolated plugins** that allow users to extend the application without modifying its source code. Plugins can be written in **any programming language** (Python, Rust, Node.js, Bash, etc.) as long as they communicate with Papr using **JSON over standard input (`stdin`) and standard output (`stdout`)**.

Each plugin runs as an independent process, providing strong isolation while keeping the plugin API simple, language agnostic, and easy to debug.

### Plugin Capabilities

A plugin requests access to specific integration surfaces through its `plugin.toml` manifest.

| Capability            | Description                                                                           |
| --------------------- | ------------------------------------------------------------------------------------- |
| `metadata-provider`   | Contribute scholarly metadata providers (e.g., querying custom library repositories). |
| `commands`            | Register custom commands that appear in the Command Palette (`Ctrl+P`).               |
| `activity-events`     | Listen to lifecycle events such as when a paper is opened, imported, or deleted.      |
| `read-paper-metadata` | Read the metadata of the currently focused paper.                                     |

A plugin only has access to the capabilities it explicitly requests.

### Plugin Actions

When invoked, Papr sends the plugin a JSON request describing the event and any available context. The plugin processes the request and returns a JSON response containing actions for Papr to execute.

Currently supported actions include:

| Action              | Description                                         |
| ------------------- | --------------------------------------------------- |
| `notify`            | Display a non-blocking notification toast.          |
| `add_to_collection` | Automatically assign a paper to a named collection. |

This request-response model keeps plugins simple while allowing the core application to remain in control.

### Example Plugins

#### Auto Tagger (Python)

Automatically categorizes newly added papers into collections based on title or abstract keywords.

**Capabilities**

* `activity-events`
* `read-paper-metadata`

#### Slack Notifier (Shell Script / cURL)

Sends a Slack webhook notification whenever you open or finish reading a paper.

**Capabilities**

* `activity-events`
* `read-paper-metadata`

### Writing a Plugin

This example demonstrates how to create the **Auto Tagger** plugin in Python.

#### Step 1 — Create the Plugin Directory

Papr searches for plugins inside its platform-specific data directory. On Linux:

```bash
mkdir -p ~/.local/share/papr/plugins/auto-tagger
cd ~/.local/share/papr/plugins/auto-tagger
```

#### Step 2 — Create the Manifest

Create a file named `plugin.toml`.

```toml
id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
api_version = 1

description = "Automatically categorizes papers into collections based on keyword rules"

executable = "tagger.py"

capabilities = [
    "activity-events",
    "read-paper-metadata"
]
```

The manifest tells Papr how to launch the plugin and which capabilities it requires.

#### Step 3 — Write the Plugin

Create `tagger.py`.

```python
#!/usr/bin/env python3

import json
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        sys.exit(1)

    response = {"actions": []}

    if request.get("event") == "paper_opened":
        paper = request.get("context", {}).get("paper", {})
        title = paper.get("title", "").lower()

        if "neural network" in title or "deep learning" in title:
            response["actions"].append({
                "type": "add_to_collection",
                "name": "Machine Learning"
            })

            response["actions"].append({
                "type": "notify",
                "message": f"Added '{paper.get('title')[:30]}...' to Machine Learning"
            })

    print(json.dumps(response))


if __name__ == "__main__":
    main()
```

Make the script executable.

```bash
chmod +x tagger.py
```

#### Step 4 — Enable the Plugin

Open the **Settings** workspace.

Press **Right Arrow** or **Enter** to focus the embedded configuration editor, then press `i` to enter Insert mode. Add the plugin identifier to the `enabled_plugins` array:

```toml
enabled_plugins = ["auto-tagger"]
```

Press `Esc`, type `:w`, and press `Enter` to save the configuration. The plugin will be discovered and loaded immediately without restarting Papr.

### Plugin Directory Layout

```text
~/.local/share/papr/plugins/
└── auto-tagger/
    ├── plugin.toml
    └── tagger.py
```

### Communication Model

Plugins communicate exclusively through JSON over `stdin` and `stdout`.

For every invocation, Papr sends a JSON request describing the event and any available context. The plugin processes the request and returns a JSON response containing one or more actions for Papr to execute.

This architecture makes plugins:

* Language independent
* Process isolated
* Easy to develop and debug
* Safe to distribute independently of the core application


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
* **Reading Queue:** A prioritized list of papers you plan to read next. Supports reordering/prioritization with `K`/`J` (or Shift/Ctrl + Up/Down) and toggling with `a` key.
* **Collections:** Paper groups mapping directly to subdirectories in your library.
* **Bookmarks:** Quick-access list of bookmarked local PDFs.
* **Authors:** Browse local papers grouped by author name.
* **Notes:** Search and browse all your paper-linked Markdown notes.
* **Downloads:** Active, completed, and failed downloads. Supports `B`, `n`, `s`, and `R` actions on completed downloads.
* **History:** A chronological log of searches, downloads, and paper opens.
* **Statistics:** Detailed reading streaks, totals, top dimensions, and a 12-week reading heatmap.
* **Settings:** Quick summary of active paths, configurations, and plugins.
* **Credits:** Redesigned interactive Credits/About page for Papr.

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
| `u` | Set the current state of the paper as unread |
| `a` | Toggle paper queue/dequeue status |
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

### Library, Reading Queue, Collections, Bookmarks, Authors

| Key | Action |
| --- | --- |
| `Enter` / `Right` / `l` | Open the PDF in your default viewer |
| `r` | Scan library folders for new files |
| `n` | Edit Markdown note |
| `s` | Move PDF file to a collection folder |
| `B` | Toggle bookmark |
| `>` | Toggle local search |
| `R` | Rename a PDF file or collection folder |
| `x` | Delete a PDF file or collection folder |
| `c` | Copy citation |

### Downloads Tab

| Key | Action |
| --- | --- |
| `Enter` / `l` | Open the downloaded PDF (once completed) |
| `B` | Toggle bookmark on the downloaded paper |
| `n` | Edit Markdown note for the downloaded paper |
| `s` | Move the downloaded paper to a collection |
| `R` | Rename the downloaded PDF |
| `x` | Delete the downloaded PDF |
| `>` | Toggle local search |
| `c` | Copy citation |

### Internal PDF Viewer

| Key | Action |
| --- | --- |
| `Esc` / `q` | Exit the PDF viewer |
| `j` / `Down` | Scroll down |
| `k` / `Up` | Scroll up |
| `PageDown` | Scroll down by a page |
| `PageUp` | Scroll up by a page |

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
