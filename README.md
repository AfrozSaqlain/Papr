<p align="center">
  <a href="https://github.com/AfrozSaqlain/Papr">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="./assets/logo-dark.png">
      <source media="(prefers-color-scheme: light)" srcset="./assets/logo-light-2.png">
      <img src="./assets/logo-light.png" alt="Papr Logo" width="280">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/papr2.gif" alt="Papr Demo" width="900">
</p>

<p align="center">
  <a href="https://crates.io/crates/papr-tui"><img src="https://img.shields.io/crates/v/papr-tui.svg?style=flat-square&color=orange" alt="Crates.io"></a>
  <a href="https://github.com/AfrozSaqlain/Papr/releases"><img src="https://img.shields.io/github/v/release/AfrozSaqlain/Papr?style=flat-square&color=blue" alt="Latest Release"></a>
  <a href="https://github.com/AfrozSaqlain/Papr/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg?style=flat-square" alt="License"></a>
</p>

Papr is an all-in-one terminal workspace engineered for academic researchers. It unifies daily research paper feed, arXiv search, local PDF library management, in-terminal reading, Markdown note-taking, and manuscript editing with LaTeX and Typst into a cohesive, keyboard-driven interface. By bringing discovery, reading, and writing into a single distraction-free environment, Papr eliminates context-switching between browsers, file managers, PDF viewers, and external editors.

Under the hood, Papr is built in Rust which gives researchers sub-millisecond interface responsiveness, low memory overhead, and safe concurrent execution for background downloads, directory indexing, and compilation without freezing the UI. With ACID-compliant local storage, full offline capability, and zero runtime dependencies, your research library remains fast, stable, and completely under your control.

---

## Key Features

### Discovery & Research Feed
* **Daily Feed & Analytics:** Delivers 10 randomized papers daily matching your research keywords while tracking streaks, reading time, and disk usage.
* **Integrated arXiv Search:** Browse arXiv by title, author, category, abstract, or DOI with paginated results and live in-view filtering (`>`).
* **Clean Naming:** Saves downloads with human-readable titles (not raw IDs).
* **GitHub Library Sync:** Synchronizes your paper catalog, reading states, and metadata to GitHub for seamless cross-device access.

### Library & Organization
* **Filesystem-Synced Groups:** Organize papers into collections that directly mirror folder directories on disk.
* **Automatic Author Cataloging:** Automatically groups your entire local library by author name.
* **Flexible Sorting & Bookmarks:** Order papers by newest, oldest, or download date; toggle bookmarks with a single keypress (`B`).
* **Activity History:** Inspect a chronological audit log of recent searches, downloads, and compilation events.

### Reading & Annotation
* **In-Terminal PDF Viewing:** Read papers directly in the terminal via Kitty or Sixel graphics protocols, or launch external viewers (Zathura, Okular).
* **Reading Progress & Smart Queue:** Remembers your exact page to resume where you left off; queued papers auto-complete only upon reaching the final page.
* **Markdown Notes:** Write per-paper annotations with a Vim-inspired editor and a live styled preview (`Tab`).
* **Productivity Stats:** Track active reading streaks, session duration, top authors, and a 12-week activity heatmap.

### Writing Studio (LaTeX & Typst)
* **Embedded Typst Engine:** Zero-setup compiler with bundled fonts, local module imports, and cached Typst Universe (`@preview`) packages.
* **LaTeX Support:** Asynchronous background compilation powered by `latexmk`.
* **Split-Pane PDF Preview & Diagnostics:** Live preview alongside the editor with inline diagnostics, source line jumps, and build logs.
* **BibTeX Citation Importer:** Search local papers or arXiv from inside the editor (`Ctrl+F`) to insert citation keys directly.
* **Project File Tree:** Navigate, create, rename, and manage multi-file manuscripts using Vim keybindings.

### Customization & Extensibility
* **19 Built-in Themes & Transparency:** Includes Catppuccin, Tokyo Night, Gruvbox, Nord, and Dracula, with transparent background support.
* **Process-Isolated Plugins:** Extend functionality via sandboxed JSON plugins (includes an out-of-the-box auto-tagger).

---

## Installation

Pre-compiled binaries are available for Linux, macOS, and Windows. Choose your preferred installation method below.

### Quick Installers

**Linux & macOS (Shell Script):**
```sh
curl --proto '=https' --tlsv1.2 -sSfL https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.sh | sh
```

**Windows (PowerShell):**
```powershell
Set-ExecutionPolicy -Scope Process Bypass
irm https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.ps1 | iex
```
*Installs `papr.exe` to `%USERPROFILE%\.cargo\bin`. Restart PowerShell if the command is not immediately recognized.*

---

### Package Managers

#### Cargo (crates.io)
If you have the Rust toolchain installed:
```sh
cargo install papr-tui
```
*(The crate is published as `papr-tui` and installs the binary `papr`.)*

#### Arch Linux (AUR)
```sh
yay -S papr-bin
# or
paru -S papr-bin
```

#### Fedora (COPR)
```sh
sudo dnf copr enable afrozsaqlain/Papr
sudo dnf install papr-tui
```
*To update an existing installation: `sudo dnf upgrade --refresh papr-tui`*

#### macOS (Homebrew Tap)
```sh
brew tap AfrozSaqlain/tap
brew trust AfrozSaqlain/tap
brew install papr
```

#### Debian, Ubuntu, & RPM Packages
Download pre-built `.deb` or `.rpm` packages from the [GitHub Releases](https://github.com/AfrozSaqlain/Papr/releases/latest):

* **Debian / Ubuntu / Mint:**
  ```sh
  sudo apt install ./papr_*.deb
  ```
* **Fedora / RHEL:**
  ```sh
  sudo dnf install ./papr-*.rpm
  ```
* **openSUSE:**
  ```sh
  sudo zypper install ./papr-*.rpm
  ```

---

## System Dependencies & Extra Setup

### Recommended Tools
While Papr runs out of the box, certain features rely on standard system utilities:

| Feature | Dependency | Package Name |
| :--- | :--- | :--- |
| **In-terminal PDF Viewer** | Kitty or Sixel terminal + Poppler | `poppler-utils` (`pdftoppm`) |
| **PDF Metadata Extraction** | Poppler | `poppler-utils` (`pdfinfo`, `pdftotext`) |
| **LaTeX Compilation** | TeX Live + Latexmk | `texlive`, `latexmk` |
| **Typst Compilation** | Embedded in Papr | *No external dependencies required* |
| **System Clipboard (Linux)** | Wayland or X11 utility | `wl-clipboard` (Wayland) or `xclip` (X11) |
| **External PDF Viewer** | Optional desktop viewer | `zathura`, `okular`, or system default |

### **In-Terminal PDF Viewer Setup (Kitty)**

For the best reading experience inside the terminal, use the [Kitty](https://sw.kovidgoyal.net/kitty/) terminal emulator:

* **Linux:**
  ```sh
  curl -L https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin
  ln -sf ~/.local/kitty.app/bin/kitty ~/.local/kitty.app/bin/kitten ~/.local/bin/
  ```
* **macOS:**
  ```sh
  curl -L https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin
  ```

> [!NOTE]
> On macOS, configure Kitty to pass the Option key as Alt so window pane shortcuts work correctly: add `macos_option_as_alt yes` to `~/.config/kitty/kitty.conf`.

### **Install other system dependencies:**

* **Ubuntu / Debian:**
  ```sh
  sudo apt update && sudo apt install -y poppler-utils wl-clipboard texlive latexmk xdg-utils
  ```
* **Fedora:**
  ```sh
  sudo dnf install -y poppler-utils wl-clipboard texlive-scheme-basic latexmk xdg-utils
  ```
* **Arch Linux:**
  ```sh
  sudo pacman -S --noconfirm poppler wl-clipboard texlive-basic texlive-latexmk xdg-utils
  ```
* **macOS:**
  ```sh
  brew install poppler basictex
  sudo tlmgr update --self && sudo tlmgr install latexmk
  ```

#### Windows Notes
The internal Kitty terminal viewer is unavailable on Windows. By default, Papr launches external PDFs using Microsoft Edge (or your configured viewer). For LaTeX authoring on Windows, install MiKTeX and Strawberry Perl as detailed in [docs/windows.md](docs/windows.md).

---

<details>
<summary><strong>Building from Source</strong></summary>

### Prerequisites
* Rust 1.92+ with Cargo ([rustup.rs](https://rustup.rs/))
* C compiler and linker (`build-essential`, `gcc`, or Xcode command-line tools)
* `pkg-config` (Linux)

### Build & Install
```sh
git clone https://github.com/AfrozSaqlain/Papr.git
cd Papr
cargo install --path crates/papr
```

Ensure `~/.cargo/bin` is in your shell `PATH`:
```sh
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

</details>

---

## User Guide

Launch Papr by running:
```sh
papr
```

### Workspaces Overview
Papr organizes your research lifecycle into 14 specialized workspaces, accessible via the left sidebar or the quick navigator (`Ctrl+B`):

| Workspace | Purpose | Primary Actions |
| :--- | :--- | :--- |
| **Dashboard** | Daily arXiv feed tailored to your interests, reading streak, and activity stats. | `Enter` metadata, `d` download, `o` browser, `c` citation |
| **Discover** | Search arXiv by title, author, category, abstract, or DOI with query filtering. | `/` search bar, `>` filter results, `d` download |
| **Library** | Catalogs all local PDFs discovered across your configured library directories. | `Enter` read, `s` sort, `n` note, `g` group, `a` queue |
| **Reading Queue**| Prioritized backlog of papers to read next; completes upon reaching the final page. | `Shift+↑/↓` reorder, `Enter` read, `a` remove |
| **Groups** | Custom collections mirrored directly into your local filesystem folders. | `g` create/assign group, `Enter` explore papers |
| **Bookmarks** | Quick-access collection of starred papers. | `B` toggle bookmark, `Enter` read |
| **Authors** | Automatically clusters local papers by author name. | `s` sort, `Enter` view author catalog |
| **Notes** | Centralized catalog of all Markdown notes attached to your papers. | `Enter` edit note, `Tab` toggle preview |
| **Downloads** | Real-time queue showing active, completed, and failed background downloads. | `r` retry failed download |
| **Projects** | Integrated LaTeX & Typst writing studio with file tree, editor, and PDF preview. | `n` create project/file, `Alt+1..4` switch panes |
| **History** | Chronological timeline of recent searches, downloads, and compilation events. | Read-only activity audit |
| **Statistics** | Analytics tracking total reading time, streaks, top authors, and a 12-week heatmap. | Visual research insights |
| **Settings** | Configuration hub for themes, paths, research keywords, and GitHub sync. | Interactive preference management |
| **Credits** | License, maintainer attributions, and core dependency information. | `Enter` open reference links |

---

### Core Workflows

#### 1. Discovering & Downloading Papers
* **Daily Feed:** Open **Dashboard** to see today's curated recommendations based on your `dashboard_keywords`. Press `Enter` to read the abstract, `d` to start a background download, `o` to view the paper on the web, or `c` to copy its BibTeX citation.
* **arXiv Search:** Switch to **Discover** and press `/` to open the search bar. Enter search terms, categories (e.g. `cs.AI`), or authors. Use `>` to filter loaded results instantly.
* **Command Line Download:** You can also download papers directly from your shell without opening the TUI:
  ```sh
  papr download 2401.12345
  papr download https://arxiv.org/abs/2401.12345
  ```

#### 2. Reading, Queuing & Taking Notes
* **Reading Progress:** Open any paper from **Library** or **Reading Queue** with `Enter`. Papr tracks your position so you resume on the exact page you last read.
* **Managing the Queue:** Add papers to your reading backlog by pressing `a`. In the **Reading Queue** workspace, reorder items with `Shift+↑` and `Shift+↓`. A paper is automatically marked finished and cleared from the queue once you reach its final page.
* **Taking Notes:** Press `n` on any paper row to open the Markdown editor. Jot down annotations, formulas, or summaries. Press `Tab` to toggle a styled preview and `Esc` to save and exit.

#### 3. Writing Manuscripts (LaTeX & Typst)
1. Open the **Projects** workspace and press `n` to create a new project. Choose between **Typst** (default) or **LaTeX**.
2. Press `Enter` to enter the project workspace:
   * **Pane Navigation:** Use `Alt+1` (File Tree), `Alt+2` (Editor), `Alt+3` (PDF Preview), or `Alt+4` (Compiler Log).
   * **Editing:** Standard Vim motions are supported. Save with `Ctrl+S` to trigger instant background compilation.
   * **Insert Citations:** Press `Ctrl+F` inside the editor to open the citation importer, search your library, and insert formatted BibTeX citations.
   * **Error Inspection:** Compiler errors display inline diagnostic badges; inspect detailed logs in `Alt+4`.

#### 4. Synchronizing Across Machines
Enable GitHub sync to keep your research catalog consistent across devices:
1. Navigate to **Settings** or edit `config.toml`.
2. Configure your GitHub repository and Personal Access Token (or set `PAPR_GITHUB_TOKEN`).
3. Papr will sync library papers and their metadata, allowing a fresh installation on another machine to restore your library effortlessly.

---

## Keyboard Reference

<details>
<summary><strong>View Keyboard Shortcuts Reference</strong></summary>

### Global Navigation
| Key | Action |
| :--- | :--- |
| `Ctrl+b` | Open workspace navigator |
| `Ctrl+t` | Open terminal command palette |
| `?` | Toggle context-sensitive help popup |
| `/` | Focus search bar (in Discover/Dashboard) |
| `Enter / → / l` | Open selected item / drill down |
| `← / h` | Return to previous view / go up |
| `Esc` | Clear search / close active modal |
| `q` | Quit Papr (outside text inputs) |

### Paper Actions
*Applicable in Library, Reading Queue, Groups, Bookmarks, and Author views:*

| Key | Action |
| :--- | :--- |
| `Enter / l` | Open PDF in viewer |
| `o` | Open paper online in default browser |
| `a` | Toggle paper in Reading Queue |
| `B` | Toggle bookmark status |
| `n` | Open per-paper Markdown note |
| `g` | Assign paper to a Group |
| `c` | Copy BibTeX citation to clipboard |
| `s` | Cycle sort order (newest, oldest, download date) |
| `>` | Open in-view filter bar |
| `R` | Rename PDF file |
| `u` | Mark paper as unread |
| `x` | Delete PDF from disk and library |

### Reading Queue & Downloads
| Key | Action |
| :--- | :--- |
| `Shift+↑` | Move selected paper up in reading queue |
| `Shift+↓` | Move selected paper down in reading queue |
| `r` | Retry a failed download |

### Writing Projects
| Key | Action |
| :--- | :--- |
| `Alt+1` | Focus project file tree |
| `Alt+2` | Focus manuscript editor |
| `Alt+3` | Focus PDF preview pane |
| `Alt+4` | Focus compilation output logs |
| `n` | Create new file (append `/` to create directory) |
| `Ctrl+s` | Save document and trigger build |
| `Ctrl+f` | Search library and insert citation |
| `Ctrl+Shift+v` | Paste clipboard text |
| `Esc` | Return focus to file tree from editor |

*For complete editor keybindings, see the [Project Editor Reference](docs/PROJECT_EDITOR_KEYBINDINGS.md).*

</details>

---

## Configuration

Papr creates a default `config.toml` on first launch. Inspect your active paths anytime with:
```sh
papr paths
```

### Directory Locations
| Platform | Configuration File | Data & Database Directory | Projects Directory |
| :--- | :--- | :--- | :--- |
| **Linux** | `~/.config/papr/config.toml` | `~/.local/share/papr/` | `~/.local/share/papr/projects/` |
| **macOS** | `~/Library/Application Support/papr/config.toml` | `~/Library/Application Support/papr/` | `~/Library/Application Support/papr/projects/` |
| **Windows** | `%APPDATA%\papr\config.toml` | `%APPDATA%\papr\` | `%APPDATA%\papr\projects\` |

### Example `config.toml`
```toml
# Appearance
theme = "catppuccin-mocha"
transparent_background = false
startup_page = "dashboard" # dashboard, discover, library, reading_queue, projects

# PDF Viewer: "internal" for terminal graphics, or an external command
pdf_viewer = "zathura {path}"

# Storage & Library Directories
library_folders = [
  "/home/user/Documents/papers",
  "/home/user/Downloads/research"
]
download_path = "/home/user/Documents/papers"
projects_directory = "/home/user/Documents/projects"

# Research Interests for Daily Dashboard Recommendations
dashboard_keywords = "machine learning, quantum computing, computer vision"

# Writing Projects
default_project_compiler = "typst" # "typst" or "latex"

# GitHub Paper Synchronization
[sync]
enabled = false
github_repo = "username/papr-library"
branch = "main"
# github_token = "ghp_..." # Or export PAPR_GITHUB_TOKEN in your environment

# Plugins
enabled_plugins = []
```

### Themes
Papr ships with 19 built-in themes: `catppuccin-mocha` (default), `catppuccin-macchiato`, `catppuccin-frappe`, `catppuccin-latte`, `tokyo-night`, `gruvbox`, `nord`, `dracula`, `light`, `rose-pink-dark`, `rose-pink-light`, `everforest`, `kanagawa`, `one-dark`, `cyberpunk`, `ember`, `verdant`, `lavender`, and `parchment`. Themes can be previewed live in the **Settings** workspace.

---

## Docker Setup

<details>
<summary><strong>View Docker Instructions (Wayland, X11, macOS)</strong></summary>

Papr can be containerized to run in an isolated environment while bind-mounting your local library and projects.

### Build Container
```sh
docker build \
  --build-arg UID="$(id -u)" \
  --build-arg GID="$(id -g)" \
  --tag papr:latest .
```

### Run Container

First, initialize host directories:
```sh
mkdir -p .papr/config .papr/data papers projects
```

**Wayland:**
```sh
docker run --rm -it \
  --name papr \
  --env WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
  --env XDG_RUNTIME_DIR=/run/user/1000 \
  --env DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/1000/bus" \
  --env XDG_CURRENT_DESKTOP=GNOME \
  --mount "type=bind,src=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY,dst=/run/user/1000/$WAYLAND_DISPLAY,readonly" \
  --mount "type=bind,src=/run/user/$(id -u)/bus,dst=/run/user/1000/bus,readonly" \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  --mount "type=bind,src=$PWD/projects,dst=/projects" \
  papr:latest
```

**X11 (Linux):**
```sh
docker run --rm -it \
  --name papr \
  --env DISPLAY=$DISPLAY \
  --env DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/1000/bus" \
  --env XDG_CURRENT_DESKTOP=GNOME \
  --mount "type=bind,src=/tmp/.X11-unix,dst=/tmp/.X11-unix,readonly" \
  --mount "type=bind,src=/run/user/$(id -u)/bus,dst=/run/user/1000/bus,readonly" \
  --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
  --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
  --mount "type=bind,src=$PWD/papers,dst=/papers" \
  --mount "type=bind,src=$PWD/projects,dst=/projects" \
  papr:latest
```

**macOS (via XQuartz):**
1. Install XQuartz: `brew install --cask xquartz`
2. Open XQuartz preferences (**Security** > check **"Allow connections from network clients"**) and restart XQuartz.
3. Allow localhost access: `xhost + 127.0.0.1`
4. Run:
   ```sh
   docker run --rm -it \
     --name papr \
     --env DISPLAY=host.docker.internal:0 \
     --mount "type=bind,src=$PWD/.papr/config,dst=/home/papr/.config/papr" \
     --mount "type=bind,src=$PWD/.papr/data,dst=/home/papr/.local/share/papr" \
     --mount "type=bind,src=$PWD/papers,dst=/papers" \
     --mount "type=bind,src=$PWD/projects,dst=/projects" \
     papr:latest
   ```
   *(Note: Native Homebrew installation is recommended on macOS for seamless browser link handling and clipboard support.)*

</details>

---

## Architecture & CLI

Papr is engineered as a modular Cargo workspace:
* **[`papr-core`](crates/papr-core):** Manages SQLite migrations, database transactions, background download scheduling, recursive library indexing, embedded Typst compilation, and external plugin execution.
* **[`papr`](crates/papr):** Manages the terminal user interface lifecycle (Ratatui + Crossterm), event loop dispatch, graphics rendering protocols, and project editor states.

### Command-Line Interface
```sh
papr                         # Launch the interactive terminal workspace
papr paths                   # Display active configuration, database, and library paths
papr download <ARXIV-ID>     # Download a paper by arXiv ID, URL, or Markdown link
papr plugin <ID> <EVENT>     # Execute a plugin event directly via JSON protocol
```

---

## Contributing

Contributions, feature requests, and bug reports are welcome! Please ensure all code quality checks pass before opening a pull request:

```sh
# Format check
cargo fmt --all -- --check

# Lints
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Test suite
cargo test --workspace

# Documentation
cargo doc --workspace --no-deps
```

Refer to [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/PLUGINS.md](docs/PLUGINS.md) for deeper technical details.

---

## License

Papr is open-source software licensed under the [MIT License](LICENSE).
