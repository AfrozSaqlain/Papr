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
  <img src="assets/papr.gif" alt="Demo" width="900">
</p>

Papr is a fast, terminal based workspace built for academic research. It brings together arXiv paper discovery, local PDF library management, reading, markdown note-taking, automatic author-based paper organization, and LaTeX manuscript editing into one terminal interface.

Instead of context-switching between web browsers, reference managers, PDF viewers, and text editors, Papr merges online search with local file organization into a distraction-free environment. Under the hood, it's built with Rust using Ratatui, Tokio, Reqwest, and SQLite, giving you a lightweight tool that runs entirely offline and keeps all your data under your control.

---

## Why Rust?

Papr was written in Rust because an academic research workspace needs to be fast, reliable, and light on system resources:
* **Performance:** Instant application startup and rapid indexing of thousands of local PDFs.
* **Async Concurrency:** Powered by Tokio's async runtime, Papr downloads papers, watches filesystem changes, and handles network calls in the background without freezing the UI.
* **Stability & Memory Safety:** Rust's strict safety guarantees eliminate memory leaks, race conditions, and unexpected crashes mid-session.
* **Reliable Local Storage:** Everything is stored locally in SQLite with Write-Ahead Logging (WAL) enabled, ensuring fast, ACID-compliant database operations that won't corrupt your library.
* **Zero-Overhead Single Binary:** Compiles down to a single, self-contained binary with minimal CPU and memory usage (no heavy runtimes or Electron bloat).

---

## Key Features

### Dashboard & Discovery
* **Daily Research Dashboard:** Displays 10 new random papers daily into your feed, from your predefined keywords, which you can customize in the **Settings** workspace to match your field of interest, helping you stay up to date with the latest advancements in your research area. Also lets you track reading time, active streaks, disk usage, and much more.
* **Integrated arXiv Search:** Search arXiv directly by title, author, category, abstract, or DOI. Search results load incrementally, let you filter on the fly.
* **Clean Filenames:** Downloads run in the background and are automatically saved with sanitized, human-readable titles instead of cryptic arXiv IDs (e.g., `2401.12345.pdf`).
* **Smart Deduplication:** Resolves database conflicts across your library (matching arXiv ID, file path, or DOI) and merges records during downloads or scans without losing your existing notes, bookmarks, or reading progress.
* **Workspace & Disk Sync:** Organizing papers into groups inside the TUI automatically mirrors those changes in your local folder structure on disk.

### Reading & Note-taking
* **In-Terminal PDF Viewing:** Read papers directly inside the terminal with smooth scrolling using Kitty or Sixel graphics protocols.
* **External Viewer & Browser Integration:** Open PDFs in external readers like Zathura or Okular (while Papr continues tracking your reading time) or jump straight to the paper in your web browser.
* **Markdown Annotations:** Write dedicated per-paper notes using a built-in Vim-inspired markdown editor with a live styled preview.
* **Reading Queue:** Prioritize your reading list with a dedicated, sortable queue.

### LaTeX Integration
* **Built-in Writing Workspace:** Create, edit, and compile LaTeX manuscripts directly within the TUI.
* **Real-time Compilation:** Asynchronous background compilation powered by `latexmk`.
* **PDF-First Layout:** Work with the live PDF preview side-by-side with your editor, and also track build logs.
* **Smart Diagnostics:** LaTeX compilation errors and warnings are cleanly grouped with exact source locations, code snippets, diagnostic hints, and a raw log view.
* **Full Project File Management:** Navigate project tree structures, create nested files and directories, rename entries, and edit code with familiar Vim-style keybindings.

### Extensibility
* **Process-Isolated Plugins:** Extend workflows using language-agnostic, versioned JSON plugins running in isolated processes with bounded execution limits. Comes with an opt-in auto-tagging plugin out of the box.

---

## Installation Guide

### General System Requirements

* A 64-bit Linux, macOS, or Windows system supported by Rust.
* A modern terminal emulator with ANSI color and Unicode support (minimum **58 columns × 18 rows**).
* Read/write access to Papr's configuration and data directories (you can although change data directories in Settings).
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

> **Note:** Some features on Windows may not work, such as the internal PDF viewer, since the Kitty terminal is not available on Windows. However, you can still use your preferred PDF viewer to open PDFs. By default, PDFs are launched in Microsoft Edge (which you can change to Chrome, Brave, or any other PDF viewer), as Microsoft Edge is available on all Windows systems.

> Windows users: Visual Studio Build Tools are only needed when building from source; MiKTeX (including `latexmk`) and Strawberry Perl are the Windows LaTeX workspace prerequisites documented in [docs/windows.md](docs/windows.md).

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

> **Note:** Currently AUR is undergiong maintenance so please use `Shell Script Installer` method if you are on Arch Linux BTW!!!

## Cargo (crates.io)
If you already have Rust installed, install Papr directly from crates.io (the crate is published under `papr-tui` and installs the binary command `papr`):

```sh
cargo install papr-tui
```

## Arch Linux (AUR)
Arch Linux and Manjaro users can install the pre-compiled binary package from the Arch User Repository:

```sh
yay -S papr-bin
```
or
```sh
paru -S papr-bin
```

## macOS Homebrew Tap
Install Papr on macOS using Homebrew:

```sh
brew tap AfrozSaqlain/tap
brew trust AfrozSaqlain/tap
brew install papr
```

## Shell Script Installer (Linux & macOS)
Install the latest pre-compiled release binary automatically:

```sh
curl --proto '=https' --tlsv1.2 -sSfL https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.sh | sh
```

## PowerShell Installer (Windows)

Run PowerShell as a user and execute:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
irm https://github.com/AfrozSaqlain/Papr/releases/latest/download/papr-tui-installer.ps1 | iex
```

This installs `papr.exe` to:

```text
%USERPROFILE%\.cargo\bin
```

If `papr` is not recognized after installation, restart your PowerShell window so the updated `PATH` is loaded.



## Debian / Ubuntu (.deb) & Fedora (.rpm) Packages
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

* **Sidebar Navigation:** Use the left sidebar to move through different workspaces and open the one you want.
* **Workspaces:** Each workspace lets you interact with papers, notes, downloads, groups, reading queues, and projects from one place. Press `Ctrl+B` to open **Browse Papr** or `Ctrl+T` for the terminal command palette. [Note: When a project is open, commands in the terminal command palette runs from that project's directory.]

Here is a complete breakdown of every section in Papr, what it does, and how to use it:

| Workspace | Purpose & Description | How to Use |
| :--- | :--- | :--- |
| **Dashboard** | Shows your main research overview, including a daily arXiv feed, reading stats, streaks, and disk usage. | This is the default home screen. Set `dashboard_keywords` in `config.toml` to tailor your feed. Open a papepr's metadata with `Enter`, use `d` to download, `o` to open paper in browser, and `c` to just copy its citation. |
| **Discover** | Lets you search arXiv by title, author, category, abstract, or DOI from within Papr. | Press `/` to go to search bar, type a query, and press `Enter`. Filter the results by pressing `>` and typing the keyword. Open a papepr's metadata with `Enter`, use `d` to download, `o` to open paper in browser, and `c` to just copy its citation. |
| **Library** | Lists all local PDF files found in your configured library directories. |  See [Papr Actions](#paper-actions) for the working keybindings. |
| **Reading Queue** | Holds the prioritized backlog of papers you plan to read next. |  See [Papr Actions](#paper-actions) and [Reading Queue](#queue) for the working keybindings.  |
| **Groups** | Organizes papers into filesystem-synced folders and collections. |  See [Papr Actions](#paper-actions) and [Groups](#groups) for the working keybindings. |
| **Bookmarks** | Collects papers you marked for quick retrieval later. | See [Papr Actions](#paper-actions) for the working keybindings. |
| **Authors** | Groups all papers in your local library by author name automatically. | Select an author from the list to view every paper written by them in your library. |
| **Notes** | Provides a searchable catalog of Markdown notes written for your papers. | Press `n` on any paper to open the Markdown editor. Press `Tab` to switch to live preview, `Esc` to save adn exit. |
| **Downloads** | Tracks active, completed, and failed background PDF downloads in real time. | See [Papr Actions](#paper-actions) and [Downloads](#downloads) for the working keybindings. |
| **Projects** | Lets you write LaTeX documents, compile `main.tex`, preview PDFs, and inspect build issues. | See [Project List](#project-list), [Project File Tree](#project-file-tree), [Project Panes](#project-panes), and [Project Editor](#project-editor-(normal-mode)) for the working keybindings. |
| **History** | Logs a chronological timeline of recent activity, searches, downloads, and project builds. | This workspace is currently read-only and intended for viewing insights. **Any ideas are welcome.** |
| **Statistics** | Shows reading habits, reading streak, total and average reading time, most readd author, most read journal, and a 12-week activity heatmap. | Track your research productivity and reading habits over time. This workspace is currently read-only and intended for viewing insights. **But ideas are welcome.** |
| **Settings** | Provides an interactive workspace for preferences, paths, themes, and plugins. | Customize themes (with live preview), dashboard keywords, library/download/project paths, and plugin settings. Follow on-screen instructions. |
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

### Project Panes
| Key | Action |
| :--- | :--- |
| `Alt+1` | focus file tree |
| `Alt+2` | focus editor |
| `Alt+3` | focus PDF preview |
| `Alt+4` | focus compiler output |

### Project Editor (Normal Mode)
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
