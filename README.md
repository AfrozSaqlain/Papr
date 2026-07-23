# Papr

<p align="center">
  <img src="assets/papr.gif" alt="Demo" width="900">
</p>

**Papr** is a fast, keyboard-first terminal workspace for academic research written in Rust. It unifies arXiv paper discovery, local library organization, reading, note-taking, and even LaTeX manuscript compilation into a single, cohesive interface.

By merging online arXiv discovery with local file organization, Papr provides a distraction-free, keyboard-driven environment designed for focused academic research. The application is built using Ratatui, Tokio, Reqwest, and SQLite, delivering a lightweight, offline-capable utility that operates entirely under your control.

---

## 🦀 Why Rust?

Papr is implemented in Rust to meet the performance, reliability, and security demands of a modern academic workflow:
* **Performance:** Instant application startup and rapid indexing of thousands of local PDFs.
* **Asynchronous Concurrency:** Leverages Tokio's runtime to run background downloads, watch filesystem changes, and perform network requests concurrently without freezing the user interface.
* **Safety and Stability:** Prevents common runtime issues such as memory corruption, race conditions, and application crashes.
* **Local Persistence:** Uses SQLite with Write-Ahead Logging (WAL) enabled, providing fast, ACID-compliant database operations.
* **Zero-Runtime Overhead:** Compiles into a single, self-contained binary with low CPU and memory footprints.

---

## 🚀 Key Features

### Discovery & Organization
* **Integrated arXiv Search:** Search the arXiv repository by title, author, category, abstract, or DOI. View full metadata, category listings, and journal references directly.
* **Automatic Title Sanitization:** Background downloads are saved with clean, cross-platform filenames rather than opaque arXiv identifiers.
* **Smart Deduplication:** Resolves database conflicts (arXiv ID, file path, DOI) by automatically merging records (preserving notes, bookmarks, and progress) during downloads and scans.
* **Workspace Syncing:** Move papers into groups within the TUI to automatically reorganize files on your disk.
* **Storage Statistics:** Monitor reading times, streaks, and disk usage through the built-in dashboard. Folders are canonicalized to prevent duplicate PDF counts.

### Reading & Note-taking
* **Built-in Terminal PDF Viewer:** View papers directly in your terminal with high-performance smooth scrolling (requires Kitty or Sixel graphics support).
* **External Viewer Support:** Seamlessly launch PDFs in your preferred desktop viewer (e.g., Zathura, Okular) with reading time tracked.
* **Markdown Annotation:** Write dedicated notes for each paper with a built-in Vim-inspired editor and live styled preview.
* **Reading Queue:** Prioritize your backlog with a dedicated, sortable reading queue (supports reordering with `K`/`J` or `Shift`/`Ctrl` + `Up`/`Down`).

### LaTeX Integration
* **Integrated Writing Workspace:** Create, edit, and compile LaTeX manuscripts directly within the TUI.
* **Real-time Compilation:** Background compilation via `latexmk`.
* **Split-pane View:** Side-by-side terminal PDF preview, file tree, source editor, and build logs.

### Extensibility
* **Process-Isolated Plugins:** Extend functionality via language-agnostic plugins communicating over a versioned JSON RPC protocol.

---

## 📦 Requirements

### System Requirements
* A 64-bit Linux, macOS, or Windows system supported by Rust.
* A modern terminal emulator with ANSI color and Unicode support (minimum **58 columns × 18 rows**).
* Read/write access to Papr's configuration and data directories.
* An internet connection (for arXiv search, metadata enrichment, and downloads). Local browsing works offline.

### Dependencies

**Required (to build from source):**
* **Rust** (1.85 or newer) and Cargo (install via [rustup](https://rustup.rs/)).
* Standard C compiler and system linker.
* **Linux Specific:** `pkg-config` and `xdg-utils`.

**Optional Feature Dependencies:**

| Feature | Dependencies |
| :--- | :--- |
| **Terminal PDF Viewer** | A Kitty- or Sixel-capable terminal, plus `poppler` (specifically `pdftoppm`). |
| **PDF Metadata & Text** | `poppler` (specifically `pdfinfo` and `pdftotext`). |
| **LaTeX Workspace** | `latexmk` and a TeX distribution (e.g., TeX Live). |
| **Linux Clipboard** | `wl-clipboard` (Wayland) or `xclip` (X11). Native `arboard` fallback is included if these are unavailable. |
| **External PDF Viewer** | The configured viewer command and its required desktop environment integration. |

**Platform-Specific Installation Commands:**

<details open>
<summary><b>Ubuntu / Debian</b></summary>

```sh
sudo apt install build-essential pkg-config xdg-utils poppler-utils wl-clipboard texlive latexmk
```
</details>

<details open>
<summary><b>Fedora</b></summary>

```sh
sudo dnf install gcc make pkgconf-pkg-config xdg-utils poppler-utils wl-clipboard texlive-scheme-basic latexmk
```
</details>

<details open>
<summary><b>Arch Linux</b></summary>

```sh
sudo pacman -S base-devel pkgconf xdg-utils poppler wl-clipboard texlive-basic texlive-latexmk
```
</details>

<details open>
<summary><b>macOS</b></summary>

First, install the Xcode Command Line Tools:
```sh
xcode-select --install
```
Then, install the remaining packages using Homebrew:
```sh
brew install poppler wl-clipboard basictex
```
*(Note: macOS natively provides `open` and clipboard access).*
</details>

<details open>
<summary><b> Windows</b></summary>

Install the **MSVC C++ Build Tools** (or Visual Studio with the *Desktop development with C++* workload) before building with Rust.
</details>

---

## 🛠️ Installation

### 1. Build and Install from Source

The recommended way to install Papr is by building it directly from the repository.

```sh
git clone https://github.com/AfrozSaqlain/Papr.git
cd Papr
cargo install --path crates/papr
```

Ensure your Cargo binary directory is in your `PATH`:
```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

### 2. Run Without Installing

```sh
cargo run --release --bin papr
```

### 3. Pre-compiled Binaries

Download precompiled binaries directly from the [GitHub Releases](https://github.com/AfrozSaqlain/Papr/releases) page. 

*Note: Depending on your OS, you may see security warnings on the first launch as the distributed binaries are not code-signed. Compiling from source avoids platform-specific security prompts.*

---

## ⚡ Quick Start

1. Run `papr` in your terminal to launch the application.
2. Press `/` to focus the search bar.
3. Enter a query (e.g., `author: Einstein` or `category: gr-qc`) and hit `Enter`.
4. Use `j`/`k` to navigate results and press `Enter` to open a paper's details.
5. Press `d` to download the paper.
6. Navigate to the **Downloads** tab to track progress. Once downloaded, press `Enter` to open the PDF.
7. Use `B` to toggle bookmarks, `n` to edit notes, or `g` to assign it to a group.

---

## ⚙️ Configuration

Papr generates a default configuration file (`config.toml`) on its first launch. You can find all resolved paths by running:
```sh
papr paths
```

### Path Locations
| OS | Configuration File | Database & Data | Projects Directory |
| :--- | :--- | :--- | :--- |
| **Linux** | `~/.config/papr/config.toml` | `~/.local/share/papr/` | `~/.local/share/papr/projects/` |
| **macOS** | `~/Library/Application Support/papr/config.toml` | `~/Library/Application Support/papr/` | `~/Library/Application Support/papr/projects/` |
| **Windows** | `%APPDATA%\papr\config.toml` | `%APPDATA%\papr\` | `%APPDATA%\papr\projects\` |

### Example `config.toml`
```toml
theme = "catppuccin-mocha"
startup_page = "dashboard"
pdf_viewer = "internal" # Use "internal" for the built-in terminal viewer, or a custom desktop viewer e.g. "zathura {path}"

# Paths to recursively scan for local PDFs
library_folders = [
  "/home/user/Documents/papers",
  "/home/user/Downloads/research",
]

# Destination directory for new downloads
download_path = "/home/user/Documents/papers"

# Default directory used to create and discover LaTeX writing projects
projects_directory = "/home/user/Documents/projects"

# Comma-separated interests to populate the Dashboard arXiv feed
dashboard_keywords = "machine learning, gravitational waves, astrophysics"

enabled_plugins = []
```

### PDF Viewers & Reading Time
To track reading sessions and statistics, Papr must be able to track the viewer's process.
* **Supported (tracks time):** `"internal"`, `"zathura {path}"` (do **NOT** use `--fork`), `"okular {path}"`, `"evince {path}"`, `"C:\\Path\\To\\SumatraPDF.exe {path}"`.
* **Unsupported (always reports 0s):** System launchers like `xdg-open`, macOS `open`, or background forks.

---

## ⌨️ Keyboard Reference

### Global Navigation
| Key | Action |
| :--- | :--- |
| `j` / `Down` | Move down / select next item |
| `k` / `Up` | Move up / select previous item |
| `Enter` / `l` / `Right` | Open selected item, section, or paper |
| `Left` | Return focus to sidebar |
| `h` / `Esc` | Go back to previous screen |
| `u` | Mark paper as unread |
| `a` | Toggle paper queue/dequeue |
| `/` | Start arXiv search |
| `Ctrl+B` | Open Browse Papr (Fast navigation command palette) |
| `?` | Toggle help |
| `q` | Close active popups or exit application |

### Discovery
| Key | Action |
| :--- | :--- |
| `Enter` | Open details page for selected search result |
| `Ctrl+Right` / `Ctrl+Left` | Browse next/previous page |
| `d` | Download PDF |
| `o` | Open paper webpage in default browser |
| `r` | Refresh/retry search |

### Paper Management (Library, Queue, Groups, Bookmarks, Notes, Downloads)
| Key | Action |
| :--- | :--- |
| `Enter` / `l` / `Right` | Open PDF |
| `r` | Scan folders / retry download |
| `n` | Edit Markdown notes |
| `g` | Move PDF to a group folder |
| `B` | Toggle bookmark |
| `>` | Toggle local search |
| `R` | Rename PDF or group |
| `x` | Delete PDF or group |
| `c` | Copy citation |

### LaTeX Workspace

#### Project List / Creation
| Key | Action |
| :--- | :--- |
| `n` | Create a new LaTeX project |
| `r` | Refresh projects list |
| `R` / `x` | Rename / Delete selected project |

#### Workspace Panes (Focused via `Alt` + Number)
| Key | Action |
| :--- | :--- |
| `Alt+1` | Focus **File Tree** pane (navigate files, press `Enter` to open) |
| `Alt+2` | Focus **Editor** pane (edit LaTeX source code) |
| `Alt+3` | Focus **PDF Preview** pane (scroll compiled PDF) |
| `Alt+4` | Focus **Build Logs** pane (scroll compilation output) |

#### Editor Mode (`Alt+2`)
| Key | Action |
| :--- | :--- |
| `i` | Enter Insert mode |
| `Esc` | Return to Normal mode (or exit Editor to File Tree) |
| `Ctrl+S` | Save file changes to disk |
| `h`/`j`/`k`/`l`, `w`/`b`, `0`/`$` | Vim motions in Normal mode |

### Internal PDF Viewer & Markdown Editor
* **PDF Viewer:** `Esc`/`q` to exit, `j`/`k` or `PageDown`/`PageUp` to scroll.
* **Markdown Editor:** `Tab` toggles styled preview, `Esc` saves and exits.

---

## 🎨 Themes

Papr supports over a dozen built-in themes out of the box, including:
`catppuccin-mocha` (default), `catppuccin-macchiato`, `catppuccin-frappe`, `catppuccin-latte`, `tokyo-night`, `gruvbox`, `nord`, `dracula`, `light`, `rose-pink-dark`, `rose-pink-light`, `everforest`, `kanagawa`, `one-dark`, and `cyberpunk`.

You can also define custom themes. Save a TOML file with color hex codes and point the `theme` configuration to its absolute path:
```toml
theme = "/home/user/.config/papr/my-theme.toml"
```

---

## 🧩 Plugins

Papr supports **process-isolated plugins** written in any language (Python, Node.js, Bash, etc.). Plugins interact via JSON RPC over `stdin`/`stdout`, enabling deep customization without compromising core stability. 

Plugins can inject scholarly metadata, hook into lifecycle events (e.g., auto-tagging), or register custom UI commands. See the [Plugins Documentation](docs/PLUGINS.md) for full details.

<details>
<summary><b>Click to view a Python plugin example (Auto Tagger)</b></summary>

This example categorizes papers into a "Machine Learning" group if the title matches certain keywords.

**1. Create the Plugin Directory**
```bash
mkdir -p ~/.local/share/papr/plugins/auto-tagger
cd ~/.local/share/papr/plugins/auto-tagger
```

**2. Create the Manifest (`plugin.toml`)**
```toml
id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
api_version = 1
description = "Automatically categorizes papers based on keyword rules"
executable = "tagger.py"
capabilities = ["activity-events", "read-paper-metadata"]
```

**3. Write the Plugin (`tagger.py`)**
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
Make it executable: `chmod +x tagger.py`

**4. Enable the Plugin**
In your `config.toml`, add:
```toml
enabled_plugins = ["auto-tagger"]
```
</details>

---

## 🐳 Running with Docker

The provided Dockerfile builds Papr with a disposable Rust stage and produces a lightweight runtime image that comes pre-bundled with LaTeX compilation tools (`latexmk` + TeX Live), Poppler PDF tools, Linux clipboard integration, and Zathura for optional external PDF viewing.

> [!IMPORTANT]
> Papr still requires an interactive terminal (`-it`). The built-in PDF viewer requires the **host terminal emulator** to support Kitty or Sixel graphics.

### Build the image
```sh
docker build \
  --build-arg UID="$(id -u)" \
  --build-arg GID="$(id -g)" \
  --tag papr:latest \
  .
```

### First run and persistent state
Create persistent host directories once:
```sh
mkdir -p .papr/config .papr/data papers projects
```

Run Papr interactively, binding local state:
```sh
docker run --rm -it \
  --name papr \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  --mount "type=bind,src=$PWD/projects,dst=/projects" \
  papr:latest
```

Set the container paths in `.papr/config/config.toml` (e.g. `download_path = "/papers"`).

<details>
<summary><b>Click to view Advanced GUI Viewer Setup for Docker (Wayland / X11)</b></summary>

If your terminal does not support Kitty/Sixel, you can use the bundled Zathura desktop viewer by passing your host's display socket into the container. Set `pdf_viewer = "zathura {path}"` in `config.toml`.

**Wayland (Linux)**
```sh
docker run --rm -it \
  --name papr \
  --env WAYLAND_DISPLAY \
  --env XDG_RUNTIME_DIR="/run/user/$(id -u)" \
  --mount "type=bind,src=/run/user/$(id -u),dst=/run/user/$(id -u)" \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  papr:latest
```

**X11 (Linux)**
```sh
xhost +si:localuser:"$(id -un)"
docker run --rm -it \
  --name papr \
  --env DISPLAY \
  --mount type=bind,src=/tmp/.X11-unix,dst=/tmp/.X11-unix,readonly \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  papr:latest
xhost -si:localuser:"$(id -un)"
```
</details>

---

## 🏗️ Architecture & CLI Utilities

Papr is split into two core crates:
* **`papr-core`:** Handles database migrations, SQLite queries, configuration loading, downloading, indexing local directories, and plugin execution.
* **`papr`:** Handles the terminal UI loop (Ratatui + Crossterm), input handling, async orchestration, and launching external viewers.

**CLI Tools:**
Run headless tasks right from the terminal:
```sh
papr                         # Start the TUI
papr paths                   # Print where configs, databases, and folders reside
papr index                   # Scan library folders and index new files
papr completions <SHELL>     # Generate completions (bash, zsh, fish)
papr plugins                 # Check discovered plugins and validation diagnostics
papr plugin <ID> <EVENT>     # Run plugin events manually for testing
```

---

## 🤝 Contributing

Contributions are welcome! If you would like to help improve Papr, keep these guidelines in mind:
1. Keep modifications scoped to the appropriate crate (`papr-core` or `papr`).
2. Avoid `unwrap`, `expect`, deliberate panics, or `unsafe` code in production paths.
3. Database updates require append-only migrations and corresponding tests.
4. Ratatui render functions must remain strictly pure (no external file or database reads/writes during rendering).

Before submitting a pull request, ensure all checks pass:
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

---

## 📄 License

Papr is open-source software distributed under the [MIT License](LICENSE).
