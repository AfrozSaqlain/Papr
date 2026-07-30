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
* **Integrated arXiv Search:** Search arXiv by title, author, category, abstract, or DOI. Results load incrementally, can be filtered in place, and retain cached pages for back/forward browsing.
* **Automatic Title Sanitization:** Background downloads are saved with clean, cross-platform filenames rather than opaque arXiv identifiers.
* **Smart Deduplication:** Resolves database conflicts (arXiv ID, file path, DOI) by automatically merging records (preserving notes, bookmarks, and progress) during downloads and scans.
* **Workspace Syncing:** Move papers into groups within the TUI to automatically reorganize files on your disk.
* **Daily Research Dashboard:** Monitor reading times, streaks, disk usage, and a cached daily arXiv recommendation feed. Use normalized, comma-separated interests to tailor the feed; it remains available offline after it has been cached.

### Reading & Note-taking
* **Built-in Terminal PDF Viewer:** View papers directly in your terminal with high-performance smooth scrolling (requires Kitty or Sixel graphics support).
* **External Viewer Support:** Seamlessly launch PDFs in your preferred desktop viewer (e.g., Zathura, Okular) with reading time tracked.
* **Markdown Annotation:** Write dedicated notes for each paper with a built-in Vim-inspired editor and live styled preview.
* **Reading Queue:** Prioritize your backlog with a dedicated, sortable reading queue (reorder with `Shift`/`Ctrl` + `Up`/`Down`).

### LaTeX Integration
* **Integrated Writing Workspace:** Create, edit, and compile LaTeX manuscripts directly within the TUI.
* **Real-time Compilation:** Background compilation via `latexmk`.
* **Split-pane View:** Side-by-side terminal PDF preview, file tree, source editor, and build logs.

### Extensibility
* **Process-Isolated Plugins:** Extend workflows with language-agnostic, versioned JSON plugins. Papr ships an opt-in auto-tagger and safely bounds every plugin invocation.

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

> 💡 **Ready to install?** For platform-specific terminal commands to install all required and optional dependencies on Ubuntu, Fedora, Arch Linux, macOS, or Windows, see **[Step 2 of the Installation Guide](#step-2-install-system-dependencies)** below.

---

## 🛠️ Beginner's Installation Guide

If you are new to Rust or terminal tools, follow this step-by-step walkthrough to get Papr up and running in minutes.

### Step 1: Install Rust & Build Prerequisites

Papr requires the Rust programming language (version 1.85 or newer).

1. **Run the Rust installer:**  
   Open your terminal application and run:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   When prompted, press `Enter` to accept the default options. The installer automatically configures your shell startup files (`~/.bashrc`, `~/.zshrc`, or `~/.profile`) so Rust is available whenever you open a terminal in the future.

2. **Activate Rust in your current terminal session:**  
   To use Rust immediately without closing your active terminal window, load the environment:
   ```sh
   source "$HOME/.cargo/env"
   ```

3. **Verify installation:**  
   Check that Cargo (Rust's package manager) is installed properly:
   ```sh
   cargo --version
   ```

### Step 2: Install System Dependencies

Select your operating system below and run the package command in your terminal:

<details open>
<summary><b>Ubuntu / Debian</b></summary>

```sh
sudo apt update && sudo apt install -y build-essential pkg-config xdg-utils poppler-utils wl-clipboard texlive latexmk git
```
</details>

<details open>
<summary><b>Fedora</b></summary>

```sh
sudo dnf install -y gcc make pkgconf-pkg-config xdg-utils poppler-utils wl-clipboard texlive-scheme-basic latexmk git
```
</details>

<details open>
<summary><b>Arch Linux</b></summary>

```sh
sudo pacman -S --noconfirm base-devel pkgconf xdg-utils poppler wl-clipboard texlive-basic texlive-latexmk git
```
</details>

<details open>
<summary><b>macOS</b></summary>

First, install the Xcode Command Line Tools:
```sh
xcode-select --install
```
Then, install the remaining dependencies using Homebrew:
```sh
brew install poppler basictex git
```
</details>

<details open>
<summary><b>Windows</b></summary>

Install the **MSVC C++ Build Tools** (or Visual Studio with the *Desktop development with C++* workload) before building with Rust.
</details>

<details>
<summary><b>Installing & Setting Kitty as Default Terminal (Optional / Recommended)</b></summary>

Papr's built-in PDF viewer (`pdf_viewer = "internal"`) renders crisp, high-resolution pages using the **Kitty Graphics Protocol**. 

#### 1. Installation
* **Ubuntu / Debian:** `sudo apt install kitty`
* **Fedora:** `sudo dnf install kitty`
* **Arch Linux:** `sudo pacman -S kitty`
* **macOS:** `brew install --cask kitty`

#### 2. Set Kitty as Default
* **Debian / Ubuntu:** `sudo update-alternatives --config x-terminal-emulator` (Select `kitty`)
* **GNOME:** `gsettings set org.gnome.desktop.default-applications.terminal exec 'kitty'`
* **Shell Variable:** Add `export TERMINAL="kitty"` to your `~/.bashrc` or `~/.zshrc`.
</details>

### Step 3: Clone & Install Papr

1. **Clone the repository:**
   ```sh
   git clone https://github.com/AfrozSaqlain/Papr.git
   cd Papr
   ```
2. **Build and install Papr:**
   ```sh
   cargo install --path crates/papr
   ```

### Step 4: Add Cargo to your PATH

If typing `papr` shows `command not found: papr`, Cargo's binary folder is not in your shell `PATH` yet. Add it permanently:

* **For Bash (`~/.bashrc`):**
  ```sh
  echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
  source ~/.bashrc
  ```
* **For Zsh (`~/.zshrc`):**
  ```sh
  echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
  source ~/.zshrc
  ```

---

## ⚡ Beginner Tutorial & App Workspaces

Launch Papr by typing its name in your terminal:

```sh
papr
```

### Understanding Papr's Navigation & Workspaces

Papr is divided into 14 specialized **workspaces** (sections), accessible via the sidebar menu on the left side of the screen.

* **Sidebar Navigation:** Press `Left Arrow` (or `h`) to move focus to the left sidebar, use `j`/`k` (or `Up`/`Down`) to highlight a section, and press `Enter` (or `l`) to open it.
* **Quick Switcher:** Press `Ctrl+B` anywhere to open **Browse Papr** (a fast command palette) and type the name of any section to jump directly to it. Press `Ctrl+T` for the terminal command palette; when a project is open, commands run from that project's directory.

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
| **Projects** | Integrated LaTeX writing workspace with background `latexmk` compilation and split-pane PDF preview. An open project uses the full workspace width for File Tree, Editor, Build, and (when internal viewing is enabled) PDF Preview. | Press `n` to create a project. Use `Alt+1` (File Tree), `Alt+2` (Editor with Vim mode), `Alt+3` (PDF Preview), `Alt+4` (Build Logs). |
| **History** | Logs a chronological timeline of your recent activity, searches, downloads, and project builds. | Scroll through past actions to re-open papers or review past search terms. |
| **Statistics** | Analytics on reading habits, total time, paper completion counts, and a 12-week reading activity heatmap. | Track your research productivity and reading habits over time. |
| **Settings** | An interactive settings workspace for preferences, paths, themes, and plugins. | Open it from the sidebar. Its Theme tab previews built-in themes live; General and Paths stage configuration values; Plugins enables or disables discovered plugins. Press `Enter` to apply changes or `Esc` to return to the sidebar. |
| **Credits** | Displays information about Papr's version, maintainers, open-source license, and core dependencies. | View application version metadata and component attribution. |

---

## 🔧 Troubleshooting & Setup Reference

### Resolving "Command Not Found: papr"

If typing `papr` gives a `command not found` error, Cargo's installation directory (`~/.cargo/bin`) is not listed in your shell's search path (`PATH`).

* **Temporary Fix (Current Session Only):**  
  Run this command in your active terminal:
  ```sh
  export PATH="$HOME/.cargo/bin:$PATH"
  ```
* **Permanent Fix:**  
  Save the search path into your shell configuration so it loads automatically in every new terminal window:
  * For **Bash** users:
    ```sh
    echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
    source ~/.bashrc
    ```
  * For **Zsh** users (default on macOS):
    ```sh
    echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
    source ~/.zshrc
    ```

### Configuring External PDF Viewers

If your terminal does not support Kitty/Sixel image graphics, Papr's built-in viewer will not render PDF pages. You can easily configure Papr to use your operating system's default graphical PDF viewer instead:

1. **Locate your configuration file:**
   ```sh
   papr paths
   ```
2. **Open `config.toml` in your preferred text editor** (e.g. `nano ~/.config/papr/config.toml`).
3. **Set your preferred viewer command:**
   * **Linux:** `pdf_viewer = "xdg-open"`
   * **macOS:** `pdf_viewer = "open"`
   * **Windows:** `pdf_viewer = "cmd /C start msedge \"\""`

### Locating Data, Database & Configuration Files

To view resolved paths for your configuration file, local SQLite database, downloaded papers, and projects directory, execute:

```sh
papr paths
```

---

## ⚙️ Configuration

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

`papr paths` prints the exact configuration file, SQLite database, downloads, plugins, and projects paths resolved on your machine.

### Example `config.toml`
```toml
theme = "catppuccin-mocha"
startup_page = "dashboard" # dashboard, discover, library, reading_queue, projects, and the other sidebar pages
pdf_viewer = "zathura {path}" # Or "internal" for the terminal viewer; omit {path} to append the PDF path

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

All eight keys shown above are the current configuration surface. On first launch Papr writes this file with `library_folders` and `download_path` set to its default downloads directory, `projects_directory` set to its default projects directory, and a platform default viewer (`xdg-open` on Linux, `open` on macOS, or `cmd /C start msedge \"\"` on Windows). Existing configurations that lack `projects_directory` are upgraded automatically. `dashboard_keywords` is case-insensitive, whitespace-normalized, and deduplicated before Papr fetches the daily feed.

The Settings workspace can edit built-in themes, startup page, PDF viewer, dashboard interests, paths, and the enabled-plugin list without leaving Papr. For a custom theme file, edit `config.toml` directly.

### PDF Viewers & Reading Time
To track reading sessions and statistics, Papr must be able to track the viewer's process.
* **Supported (tracks time):** `"internal"`, `"zathura {path}"`, `"okular {path}"`, `"evince {path}"`, `"C:\\Path\\To\\SumatraPDF.exe {path}"`.
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
| `/` | Start arXiv search |
| `Ctrl+B` | Open Browse Papr (Fast navigation command palette) |
| `Ctrl+T` | Open terminal command palette (`Enter` runs, `Tab` completes, `Esc` closes) |
| `?` | Toggle help |
| `q` | Exit the application outside text input |

### Discovery & Dashboard
| Key | Action |
| :--- | :--- |
| `Enter` | Open details page for selected search result |
| `Ctrl+Right` / `Ctrl+Left` | Browse next/previous page |
| `d` | Download PDF |
| `c` | Copy citation |
| `o` | Open paper webpage in default browser |
| `r` | Refresh/retry search |
| `>` | Filter the loaded results |

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
| `o` | Open the paper's arXiv page (when available) |
| `a` / `u` | Toggle the reading queue / mark a paper unread |

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

#### File Tree (`Alt+1`)
| Key | Action |
| :--- | :--- |
| `Enter` / `Right` | Open a source file or enter the selected folder |
| `Left` | Return to the parent folder; exit the project at its root |
| `n` | Create a file; end the name with `/` to create a folder |
| `R` | Rename the selected file or folder |
| `x` | Confirm then delete the selected file or folder |
| `Esc` | Return to the project list from the project root |

The File Tree shows folders, LaTeX/project source files, and every image format supported by Papr's image library.

#### Editor Mode (`Alt+2`)
| Key | Action |
| :--- | :--- |
| `i` | Enter Insert mode |
| `Esc` | Return to the File Tree |
| `Ctrl+S` | Save file changes to disk |
| `Ctrl+Shift+V` | Paste clipboard text exactly into an open `.bib` file and save it |
| `h`/`j`/`k`/`l`, `w`/`b`, `0`/`$` | Vim motions in Normal mode |

### Internal PDF Viewer & Markdown Editor
* **PDF Viewer:** `Esc`/`q` to exit; use `j`/`k`, `Up`/`Down`, `PageDown`/`PageUp`, or the mouse wheel to scroll.
* **Markdown Editor:** `Tab` toggles styled preview, `Esc` saves and exits.

### Settings Workspace
* **Tabs:** `Left`/`Right` switch among Theme, General, Paths, and Plugins when the tab bar is focused; `Down`/`j`/`Enter` enters a tab. `Left` on Theme returns to Navigation instead of wrapping.
* **Theme:** `j`/`k` previews built-in themes live; `Enter` applies the staged settings.
* **Lists:** In dashboard keywords and path lists, `a` adds, `d`/`Delete` removes, and `K`/`J` reorders entries. In the Plugins tab, `Space` toggles the selected plugin.
* **Apply and leave:** `Enter` writes and applies all staged changes. `Esc` stops editing a field, or returns focus to the sidebar when no field is being edited.

---

## 🎨 Themes

Papr supports over a dozen built-in themes out of the box, including:
`catppuccin-mocha` (default), `catppuccin-macchiato`, `catppuccin-frappe`, `catppuccin-latte`, `tokyo-night`, `gruvbox`, `nord`, `dracula`, `light`, `rose-pink-dark`, `rose-pink-light`, `everforest`, `kanagawa`, `one-dark`, `cyberpunk`, `ember`, `verdant`, `lavender`, and `parchment`.

You can also define custom themes. Save a TOML file with color hex codes and point the `theme` configuration to its absolute path:
```toml
theme = "/home/user/.config/papr/my-theme.toml"
```

---

## 🧩 Plugins

Papr supports **process-isolated plugins** written in any language (Python, Node.js, Bash, or an executable). A bundle lives under the plugins directory printed by `papr paths`, contains `plugin.toml`, and exchanges one JSON request and response over standard input/output. A plugin must be explicitly named in `enabled_plugins` before Papr runs it.

Papr creates the built-in **auto-tagger** bundle when the plugins directory has no plugin bundles. It is discovered but disabled by default. The Settings → Plugins tab, `enabled_plugins` in `config.toml`, and `papr plugins` all show its status and any discovery diagnostics.

Papr currently dispatches lifecycle requests for `paper_imported`, `paper_downloaded`, and `paper_opened`. Plugins can return `notify` and `add_to_collection` actions; the latter assigns the contextual local paper to a Group. The manifest also declares the protocol capabilities (`metadata-provider`, `commands`, `activity-events`, and `read-paper-metadata`). See the [plugin protocol reference](docs/PLUGINS.md) for the manifest and JSON contract.

<details>
<summary><b>Click to view the Built-In Python plugin (Auto Tagger)</b></summary>

This plugin categorizes newly imported or downloaded papers from their titles. Its supplied rules create **Machine Learning**, **Quantum Computing**, or **Computer Systems & Networking** groups; the first matching rule wins. Edit `RULES` in the generated script to tailor it.

**1. Plugin Location**
Papr automatically populates built-in plugins at `~/.local/share/papr/plugins/auto-tagger`:
```bash
cd ~/.local/share/papr/plugins/auto-tagger
```

**2. Manifest (`plugin.toml`)**
```toml
id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
api_version = 1
description = "Automatically categorizes papers based on keyword rules"
executable = "tagger.py"
capabilities = ["activity-events", "read-paper-metadata"]
```

**3. Plugin Code (`tagger.py`)**
```python
#!/usr/bin/env python3
"""
Auto Tagger Plugin for Papr
---------------------------
User Customization:
To add your own groups and filter rules, edit the `RULES` list below.
Each rule consists of:
  - "group": The name of the group in Papr
  - "keywords": Keyword, keyphrase, or regex patterns matched against titles.
"""

import json
import re
import sys

RULES = [
    {
        "group": "Machine Learning",
        "keywords": [
            r"\bneural networks?\b",
            r"\bdeep learning\b",
            r"\bmachine learning\b",
            r"\btransformers?\b",
            r"\bllms?\b",
            r"\breinforcement learning\b",
            r"\bartificial intelligence\b",
        ],
    },
    {
        "group": "Quantum Computing",
        "keywords": [
            r"\bquantum computing\b",
            r"\bqubits?\b",
            r"\bquantum algorithms?\b",
        ],
    },
    {
        "group": "Computer Systems & Networking",
        "keywords": [
            r"\bdistributed systems?\b",
            r"\boperating systems?\b",
            r"\bcloud computing\b",
            r"\bcomputer networks?\b",
        ],
    },
]

def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        sys.exit(1)

    response = {"actions": []}

    event = request.get("event")
    if event in ("paper_imported", "paper_downloaded"):
        paper = request.get("context", {}).get("paper", {})
        title = paper.get("title", "")
        if not title:
            print(json.dumps(response))
            return

        for rule in RULES:
            group_name = rule["group"]
            keywords = rule.get("keywords", [])
            if any(re.search(pattern, title, re.IGNORECASE) for pattern in keywords):
                response["actions"].append({
                    "type": "add_to_collection",
                    "name": group_name
                })
                response["actions"].append({
                    "type": "notify",
                    "message": f"Added '{title[:30]}...' to {group_name}"
                })
                break  # Stop evaluating further rules once assigned to a group

    print(json.dumps(response))

if __name__ == "__main__":
    main()
```
Make it executable: `chmod +x tagger.py`

**4. Enabling the Plugin**
In your `config.toml`, add:
```toml
enabled_plugins = ["auto-tagger"]
```
*(Plugins are disabled by default until added to `enabled_plugins`)*
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
papr plugin <ID> <EVENT>     # Invoke an enabled plugin with an empty context (use --timeout <seconds> to override 10s)
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
