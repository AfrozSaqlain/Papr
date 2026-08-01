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

### General System Requirements

* A 64-bit Linux, macOS, or Windows system supported by Rust.
* A modern terminal emulator with ANSI color and Unicode support (minimum **58 columns × 18 rows**).
* Read/write access to Papr's configuration and data directories.
* An internet connection (for arXiv search, metadata enrichment, and downloads). Local browsing works offline.

### Dependencies

**Required (to use Papr):**
* **Linux Specific:** `pkg-config` and `xdg-utils`.

**Optional Feature Dependencies:**

| Feature | Dependencies |
| :--- | :--- |
| **Terminal PDF Viewer** | A Kitty- or Sixel-capable terminal, plus `poppler` (specifically `pdftoppm`). Kitty is optional and not installed automatically. |
| **PDF Metadata & Text** | `poppler` (specifically `pdfinfo` and `pdftotext`). |
| **LaTeX Workspace** | `latexmk` and a TeX distribution (e.g., TeX Live). |
| **Linux Clipboard** | `wl-clipboard` (Wayland) or `xclip` (X11). Native `arboard` fallback is included if these are unavailable. |
| **External PDF Viewer** | The configured viewer command and its required desktop environment integration. |

**Install OS-specific prerequisites:**

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

If you want to use kitty with Papr’s terminal PDF viewer, install it separately and register it as the terminal launcher where your OS supports that:

* **Linux:**
  ```sh
  curl -L https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin
  ln -sf ~/.local/kitty.app/bin/kitty ~/.local/kitty.app/bin/kitten ~/.local/bin/
  cp ~/.local/kitty.app/share/applications/kitty.desktop ~/.local/share/applications/
  cp ~/.local/kitty.app/share/applications/kitty-open.desktop ~/.local/share/applications/
  sed -i "s|Icon=kitty|Icon=$(readlink -f ~)/.local/kitty.app/share/icons/hicolor/256x256/apps/kitty.png|g" ~/.local/share/applications/kitty*.desktop
  sed -i "s|Exec=kitty|Exec=$(readlink -f ~)/.local/kitty.app/bin/kitty|g" ~/.local/share/applications/kitty*.desktop
  echo 'kitty.desktop' > ~/.config/xdg-terminals.list
  ```
* **macOS:**
  ```sh
  curl -L https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin
  open -a kitty
  ```
  macOS does not offer a single system-wide default-terminal setting, so use kitty directly from Applications, Spotlight, or the Dock.

Select your preferred installation method below. Pre-built binaries are available for Linux, macOS, and Windows.

Color coding:

* ✳️ = Personally tested and verified.
* ❌  = Not yet personally tested (expected to work, but unverified).

## ✳️ Cargo (crates.io)
If you already have Rust installed, install Papr directly from crates.io (the crate is published under `papr-tui` and installs the binary command `papr`):

```sh
cargo install papr-tui
```

## ✳️ Arch Linux (AUR)
Arch Linux and Manjaro users can install the pre-compiled binary package from the Arch User Repository:

```sh
yay -S papr-bin
```
or
```sh
paru -S papr-bin
```

## ✳️ macOS Homebrew Tap
Install Papr on macOS using Homebrew:

```sh
brew tap AfrozSaqlain/tap
brew trust AfrozSaqlain/tap
brew install papr
```

## ❌ Shell Script Installer (Linux & macOS)
Install the latest pre-compiled release binary automatically:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.sh | sh
```

## ❌ PowerShell Installer (Windows)
Run PowerShell as user and execute:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.ps1 | iex"
```


## ❌ Debian / Ubuntu (.deb) & Fedora (.rpm) Packages
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

<details>
<summary>Building from Source</summary>

### Build Requirements
* **Rust** (1.85 or newer) and Cargo (install via [rustup](https://rustup.rs/)).
* Standard C compiler and system linker.
* **Linux Specific:** `pkg-config` and `xdg-utils`.

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

</details>

---

## Beginner Tutorial & App Workspaces

Launch Papr by typing its name in your terminal:

```sh
papr
```

### Understanding Papr's Navigation & Workspaces

Papr is divided into 14 specialized **workspaces** (sections), accessible via the sidebar menu on the left side of the screen.

* **Sidebar Navigation:** Use the left sidebar to move through different workspaces and open the one you want with the keyboard.
* **Workspaces:** Each workspace lets you interact with papers, notes, downloads, groups, reading queues, and projects from one place. Press `Ctrl+B` to open **Browse Papr** or `Ctrl+T` for the terminal command palette; `Tab` cycles completions. When a project is open, commands run from that project's directory.

Here is a complete breakdown of every section in Papr, what it does, and how to use it:

| Workspace | Purpose & Description | How to Use |
| :--- | :--- | :--- |
| **Dashboard** | Shows your main research overview, including reading stats, streaks, disk usage, and a daily arXiv feed. | This is the default home screen. Set `dashboard_keywords` in `config.toml` to tailor your feed. |
| **Discover** | Lets you search arXiv by title, author, category, abstract, or DOI from within Papr. | Press `/` to focus search, type a query, and press `Enter`. Scroll with `j`/`k`, open details with `Enter`, and use `d` to download. |
| **Library** | Lists all local PDF files found in your configured library directories. | Press `r` to scan folders. Select a paper and press `Enter` to view PDF, `n` for notes, `g` for groups, `B` for bookmarks, `R` to rename. |
| **Reading Queue** | Holds the prioritized backlog of papers you plan to read next. | Press `a` on any paper to queue or dequeue it. Reorder priority with `Shift`/`Ctrl` + `Up`/`Down`. |
| **Groups** | Organizes papers into filesystem-synced folders and collections. | Press `g` on a paper to assign it to a group. Papr creates the folder on disk and moves the PDF automatically. |
| **Bookmarks** | Collects papers you marked for quick retrieval later. | Press `B` on any paper in any workspace to toggle its bookmark status. |
| **Authors** | Groups all papers in your local library by author name automatically. | Select an author from the list to view every paper written by them in your collection. |
| **Notes** | Provides a searchable catalog of Markdown notes written for your papers. | Press `n` on any paper to open the Markdown editor. Press `Tab` to switch to live preview, `Esc` to save. |
| **Downloads** | Tracks active, completed, and failed background PDF downloads in real time. | Monitor download progress. Select a finished download and press `Enter` to open the PDF directly. |
| **Projects** | Lets you write LaTeX documents, compile `main.tex`, preview PDFs, and inspect build issues. | Press `n` to create a project. Open it with `Enter`; `main.tex` is selected in the File Tree. Use `Alt+1` (File Tree), `Alt+2` (Editor), `Alt+3` (PDF Preview), and `Alt+4` (Build). |
| **History** | Logs a chronological timeline of recent activity, searches, downloads, and project builds. | Scroll through past actions to reopen papers or review earlier search terms. |
| **Statistics** | Shows reading habits, total time, completion counts, and a 12-week activity heatmap. | Track your research productivity and reading habits over time. |
| **Settings** | Provides an interactive workspace for preferences, paths, themes, and plugins. | Open it from the sidebar. Theme previews built-in themes live; General and Paths stage values; Plugins enables or disables plugins. Press `Enter` to apply changes or `Esc` to return. |
| **Credits** | Shows Papr's version, maintainers, license, and core dependencies. | View application version metadata and component attribution. |

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

The table below summarizes the most useful keys for moving around Papr and opening the main workspaces.

### Global Navigation
| Key | Action |
| :--- | :--- |
| `?` | toggle reference |
| `/` | toggle arXiv search |
| `Ctrl+b` | toggle navigator |
| `Ctrl+t` | terminal palette |
| `Esc` | close local search |
| `Enter / → / l` | open selected item |
| `← / h` | return one level |
| `q` | quit outside text input |

### Paper Actions

Paper rows include Library, Downloads, Bookmarks, Notes, Reading Queue, and the paper lists inside Groups or Authors.

| Key | Action |
| :--- | :--- |
| `Enter / → / l` | open local PDF |
| `o` | open paper online |
| `B` | toggle bookmark |
| `>` | toggle local search |
| `n` | open note |
| `g` | assign to group |
| `R` | rename PDF |
| `c` | copy citation |
| `x` | confirm then delete PDF |
| `a` | toggle reading queue |
| `u` | mark PDF unread |

### Notes & PDF Viewer
| Key | Action |
| :--- | :--- |
| `Tab` | toggle edit / preview |
| `Esc` | save and leave note |

### Internal PDF Viewer
| Key | Action |
| :--- | :--- |
| `Esc / q` | close internal viewer |

### Credits
| Key | Action |
| :--- | :--- |
| `Enter` | open selected link |

### Discover & Dashboard
| Key | Action |
| :--- | :--- |
| `/` | toggle arXiv search |
| `d` | download paper |
| `Enter` | open downloaded PDF |

### Discover
| Key | Action |
| :--- | :--- |
| `/` | toggle arXiv search |
| `Ctrl+←` | previous cached page |
| `Ctrl+→` | next cached page |
| `r` | retry loading failure |

### Groups
| Key | Action |
| :--- | :--- |
| `g` | create a group |

### Queue
| Key | Action |
| :--- | :--- |
| `Shift+↑` | move queued paper up |
| `Shift+↓` | move queued paper down |

### Downloads
| Key | Action |
| :--- | :--- |
| `r` | retry failed download |

### Project List
| Key | Action |
| :--- | :--- |
| `n` | create named project |
| `r` | refresh project list |
| `R` | rename selected project |
| `x` | delete selected project |

### Project File Tree

Shows folders, source files, and supported image files.

| Key | Action |
| :--- | :--- |
| `Enter / →` | open file or enter folder |
| `←` | parent folder; exit at project root |
| `n` | create file; add / for folder |
| `R` | rename selected file or folder |
| `x` | confirm then delete file or folder |
| `Esc` | return to project list at root |

### Project Panes & Preview
| Key | Action |
| :--- | :--- |
| `Alt+1` | focus file tree |
| `Alt+2` | focus editor |
| `Alt+3` | focus PDF preview |
| `Alt+4` | focus compiler output |

### Project Editor — Normal
| Key | Action |
| :--- | :--- |
| `i` | enter Insert mode |
| `w / b` | next / previous word |
| `0/$, gg/G` | line/file bounds |
| `x / Delete` | delete character |
| `V, j/k, y/d` | select lines, move, yank/delete |
| `u / Ctrl+r` | undo/redo; Ctrl+Bksp/Delete word |
| `PgUp/PgDn` | page move; wheel scrolls |
| `Esc` | focus file tree |
| `Ctrl+s` | save current source |
| `Ctrl+Shift+v` | paste exactly into .tex/.bib and save |

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
