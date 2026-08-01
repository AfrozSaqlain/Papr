# Papr

<p align="center">
  <img src="assets/papr.gif" alt="Demo" width="900">
</p>

**Papr** is a fast, keyboard-first terminal workspace for academic research written in Rust. It unifies arXiv paper discovery, local library organization, reading, note-taking, automatic author-based paper organization, and LaTeX manuscript compilation into a single, cohesive terminal interface.

By merging online arXiv discovery with local file organization, Papr provides a distraction-free, keyboard-driven environment designed for focused academic research. The application is built using Ratatui, Tokio, Reqwest, and SQLite, delivering a lightweight, offline-capable utility that operates entirely under your control.

---

## Why Rust?

Papr is implemented in Rust to meet the performance, reliability, and security demands of a modern academic workflow:
* **Performance:** Instant application startup and rapid indexing of thousands of local PDFs.
* **Asynchronous Concurrency:** Leverages Tokio's runtime to run background downloads, watch filesystem changes, and perform network requests concurrently without freezing the user interface.
* **Safety and Stability:** Prevents common runtime issues such as memory corruption, race conditions, and application crashes.
* **Local Persistence:** Uses SQLite with Write-Ahead Logging (WAL) enabled, providing fast, ACID-compliant database operations.
* **Zero-Runtime Overhead:** Compiles into a single, self-contained binary with low CPU and memory footprints.

---

## Key Features

### Discovery & Organization
* **Daily Research Dashboard:** Monitor reading times, streaks, disk usage, and a cached daily arXiv recommendation feed. Use normalized, comma-separated interests to tailor the feed; it remains available offline after it has been cached.
* **Integrated arXiv Search:** Search arXiv by title, author, category, abstract, or DOI. Results load incrementally, can be filtered in place, and retain cached pages for back/forward browsing.
* **Automatic Title Sanitization:** Background downloads are saved with clean, cross-platform filenames rather than opaque arXiv identifiers.
* **Smart Deduplication:** Resolves database conflicts (arXiv ID, file path, DOI) by automatically merging records (preserving notes, bookmarks, and progress) during downloads and scans.
* **Workspace Syncing:** Move papers into groups within the TUI to automatically reorganize files on your disk.

### Reading & Note-taking
* **Built-in Terminal PDF Viewer:** View papers directly in your terminal with high-performance smooth scrolling (requires Kitty or Sixel graphics support).
* **External Viewer Support:** Seamlessly launch PDFs in your preferred desktop viewer (e.g., Zathura, Okular) with reading time tracked, or your browser.
* **Markdown Annotation:** Write dedicated notes for each paper with a built-in Vim-inspired editor and live styled preview.
* **Reading Queue:** Prioritize your backlog with a dedicated, sortable reading queue.

### LaTeX Integration
* **Integrated Writing Workspace:** Create, edit, and compile LaTeX manuscripts directly within the TUI.
* **Real-time Compilation:** Background compilation via `latexmk`.
* **PDF-first Layout:** Keep the live PDF preview beside the editor; open Build logs only when you need diagnostics.
* **Actionable Diagnostics:** Build output groups LaTeX errors and warnings with source locations, code snippets, hints, and a raw-log view.
* **Project Editing & Files:** Use Vim-style editing, create nested files or folders, rename project entries, and navigate the project tree without leaving Papr.

### Extensibility
* **Process-Isolated Plugins:** Extend workflows with language-agnostic, versioned JSON plugins. Papr ships an opt-in auto-tagger and safely bounds every plugin invocation.

---

## Installation Guide

Select your preferred installation method below. Pre-built binaries are available for Linux, macOS, and Windows.

### Cargo (crates.io)
If you already have Rust installed, install Papr directly from crates.io (the crate is published under `papr-tui` and installs the binary command `papr`):

```sh
cargo install papr-tui
```

### Arch Linux (AUR)
Arch Linux and Manjaro users can install the pre-compiled binary package from the Arch User Repository:

```sh
yay -S papr-bin
```
or
```sh
paru -S papr-bin
```

### macOS Homebrew Tap
Install Papr on macOS using Homebrew:

```sh
brew tap AfrozSaqlain/tap
brew trust AfrozSaqlain/tap
brew install papr
```

### Shell Script Installer (Linux & macOS)
Install the latest pre-compiled release binary automatically:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.sh | sh
```

### PowerShell Installer (Windows)
Run PowerShell as user and execute:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.ps1 | iex"
```


### Debian / Ubuntu (`.deb`) & Fedora (`.rpm`) Packages
Download the `.deb` or `.rpm` package from the [Latest Release Page](https://github.com/AfrozSaqlain/Papr/releases/latest) and install:

* **Debian / Ubuntu / Mint:**
  ```sh
  sudo apt install ./papr_*.deb
  ```
* **Fedora / RHEL / CentOS:**
  ```sh
  sudo dnf install ./papr-*.rpm
  ```
* **openSUSE:**
  ```sh
  sudo zypper install ./papr-*.rpm
  ```

---

## Building From Source

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

### Step 1: Install Rust & Build Prerequisites

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### Step 2: Install System Dependencies

* **Ubuntu / Debian:**
  ```sh
  sudo apt update && sudo apt install -y build-essential pkg-config xdg-utils poppler-utils wl-clipboard texlive latexmk git
  ```
* **Fedora:**
  ```sh
  sudo dnf install -y gcc make pkgconf-pkg-config xdg-utils poppler-utils wl-clipboard texlive-scheme-basic latexmk git
  ```
* **Arch Linux:**
  ```sh
  sudo pacman -S --noconfirm base-devel pkgconf xdg-utils poppler wl-clipboard texlive-basic texlive-latexmk git
  ```
* **macOS:**
  ```sh
  xcode-select --install
  brew install poppler basictex git
  ```

### Step 3: Clone & Install

```sh
git clone https://github.com/AfrozSaqlain/Papr.git
cd Papr
cargo install --path crates/papr
```

If typing `papr` shows `command not found`, add `$HOME/.cargo/bin` to your shell `PATH`:

```sh
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

---

## Beginner Tutorial & App Workspaces

Launch Papr by typing its name in your terminal:

```sh
papr
```

### Understanding Papr's Navigation & Workspaces

Papr is divided into 14 specialized **workspaces** (sections), accessible via the sidebar menu on the left side of the screen.

* **Sidebar Navigation:** Press `Left Arrow` (or `h`) to move focus to the left sidebar, use `j`/`k` (or `Up`/`Down`) to highlight a section, and press `Enter` (or `l`) to open it.
* **Quick Switcher:** Press `Ctrl+B` anywhere to open **Browse Papr** (a fast command palette) and type the name of any section to jump directly to it. Press `Ctrl+T` for the terminal command palette; press `Tab` to find or cycle command/path completions. When a project is open, commands run from that project's directory.

Here is a complete breakdown of every section in Papr, what it does, and how to use it:

| Workspace | Purpose & Description | How to Use |
| :--- | :--- | :--- |
| **Dashboard** | Serves as your main research overview. Displays reading statistics, streaks, disk storage usage, and a daily arXiv paper feed. | This is the default home screen. Set `dashboard_keywords` in `config.toml` to customize your feed. |
| **Discover** | Allows you to search the online arXiv paper repository by title, author, category, abstract, or DOI. | Press `/` to focus search, type query and press `Enter`. Scroll results with `j`/`k`, press `Enter` for details, `d` to download. |
| **Library** | Indexes and lists all local PDF files stored within your configured library directories. | Press `r` to scan folders. Select any paper and press `Enter` to view PDF, `n` for notes, `g` for groups, `B` for bookmarks, `R` to rename. |
| **Reading Queue** | Prioritized backlog where you manage papers you plan to read next. | Press `a` on any paper to queue/dequeue. Reorder priority using `Shift`/`Ctrl` + `Up`/`Down`. |
| **Groups** | Organizes your papers into filesystem-synced folders (collections). | Press `g` on a paper to assign it to a group. Papr automatically creates the folder on disk and moves the PDF. |
| **Bookmarks** | Collects all papers you have marked as bookmarked for quick retrieval. | Press `B` on any paper across any workspace to toggle its bookmark status. |
| **Authors** | Automatically groups all papers in your local library by author name. | Select an author from the list to view all papers written by them in your collection. |
| **Notes** | Displays a searchable catalog of all Markdown notes written for your papers. | Press `n` on any paper to open the Markdown editor. Press `Tab` to switch to styled live preview, `Esc` to save. |
| **Downloads** | Real-time tracker for active background PDF downloads, completed downloads, and failed requests. | Monitor download progress. Select a finished download and press `Enter` to open the PDF directly. |
| **Projects** | A place to write LaTeX documents. Papr watches the project, compiles `main.tex` in the background, shows a PDF preview, and explains compiler problems. | Press `n` to create a project. Open it with `Enter`; `main.tex` is selected in the File Tree. Use `Alt+1` (File Tree), `Alt+2` (Editor), `Alt+3` (PDF Preview), and `Alt+4` (Build). |
| **History** | Logs a chronological timeline of your recent activity, searches, downloads, and project builds. | Scroll through past actions to re-open papers or review past search terms. |
| **Statistics** | Analytics on reading habits, total time, paper completion counts, and a 12-week reading activity heatmap. | Track your research productivity and reading habits over time. |
| **Settings** | An interactive settings workspace for preferences, paths, themes, and plugins. | Open it from the sidebar. Its Theme tab previews built-in themes live; General and Paths stage configuration values; Plugins enables or disables discovered plugins. Press `Enter` to apply changes or `Esc` to return to the sidebar. |
| **Credits** | Displays information about Papr's version, maintainers, open-source license, and core dependencies. | View application version metadata and component attribution. |

---

## Configuration

Papr generates a default configuration file (`config.toml`) on its first launch. You can find all resolved paths by running:
```sh
papr paths
```

### Path Locations
| OS | Configuration File | Database, Downloads & Plugins | Projects Directory |
| :--- | :--- | :--- | :--- |
| **Linux** | `~/.config/papr/config.toml` | `~/.local/share/papr/` | `~/.local/share/papr/projects/` |
| **macOS** | `~/Library/Application Support/papr/config.toml` | `~/Library/Application Support/papr/` | `~/Library/Application Support/papr/projects/` |
| **Windows** | `%APPDATA%\papr\config.toml` | `%APPDATA%\papr\` | `%APPDATA%\papr\projects\` |

### Example `config.toml`
```toml
theme = "catppuccin-mocha"
startup_page = "dashboard" # dashboard, discover, library, reading_queue, projects, etc.
pdf_viewer = "zathura {path}" # Or "internal" for terminal viewer

# Paths to recursively scan for local PDFs
library_folders = [
  "/home/user/Documents/papers",
  "/home/user/Downloads/research",
]

# Destination directory for new downloads
download_path = "/home/user/Documents/papers"

# Default directory used to create and discover LaTeX writing projects
projects_directory = "/home/user/Documents/projects"

# Comma-separated interests to populate the daily Dashboard arXiv feed
dashboard_keywords = "machine learning, gravitational waves, astrophysics"

enabled_plugins = []
```

---

## Keyboard Reference

### Global Navigation
| Key | Action |
| :--- | :--- |
| `j` / `Down` | Move down / select next item |
| `k` / `Up` | Move up / select previous item |
| `Enter` / `l` / `Right` | Open selected item, section, or paper |
| `Left` | Return focus to sidebar |
| `h` / `Esc` | Go back to previous screen |
| `/` | Start arXiv search |
| `Ctrl+B` | Open Browse Papr (Fast navigation command palette) |
| `Ctrl+T` | Open terminal command palette (`Enter` runs, `Tab` lists/cycles completions) |
| `?` | Toggle help |
| `q` | Exit the application outside text input |

### Workspace Panes in Projects (`Alt` + Number)
| Key | Action |
| :--- | :--- |
| `Alt+1` | Focus **File Tree** pane (navigate files, press `Enter` to open) |
| `Alt+2` | Focus **Editor** pane (edit LaTeX source code) |
| `Alt+3` | Focus **PDF Preview** pane (scroll compiled PDF) |
| `Alt+4` | Focus **Build** pane (view structured compiler diagnostics) |

---

## Themes

Papr supports over a dozen built-in themes out of the box, including:
`catppuccin-mocha` (default), `catppuccin-macchiato`, `catppuccin-frappe`, `catppuccin-latte`, `tokyo-night`, `gruvbox`, `nord`, `dracula`, `light`, `rose-pink-dark`, `rose-pink-light`, `everforest`, `kanagawa`, `one-dark`, `cyberpunk`, `ember`, `verdant`, `lavender`, and `parchment`.

---

## Running with Docker

```sh
docker build \
  --build-arg UID="$(id -u)" \
  --build-arg GID="$(id -g)" \
  --tag papr:latest \
  .
```

Run Papr interactively:
```sh
docker run --rm -it \
  --name papr \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  --mount "type=bind,src=$PWD/projects,dst=/projects" \
  papr:latest
```

---

## Architecture & CLI Utilities

Papr is split into two core crates:
* **`papr-core`:** Handles database migrations, SQLite queries, configuration loading, downloading, indexing local directories, and plugin execution.
* **`papr`:** Handles the terminal UI loop (Ratatui + Crossterm), input handling, async orchestration, and launching external viewers.

**CLI Tools:**
```sh
papr                         # Start the TUI
papr paths                   # Print where configs, databases, and folders reside
papr index                   # Scan library folders and index new files
papr completions <SHELL>     # Generate completions (bash, zsh, fish)
papr plugins                 # Check discovered plugins and validation diagnostics
```

---

## Contributing

Contributions are welcome! Before submitting a pull request, ensure all checks pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

---

## License

Papr is open-source software distributed under the [MIT License](LICENSE).
