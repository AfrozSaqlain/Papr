//! Embedded settings workspace — rendering and keyboard handling.
//!
//! This module owns everything related to the four-tab settings UI:
//!   • Theme   – live-preview theme selector
//!   • General – startup_page, pdf_viewer, enabled_plugins
//!   • Paths   – library_folders (multi-entry), download_path, projects_directory
//!   • Plugins – enable / disable plugins
//!
//! All changes are staged in [`SettingsModalState`] and written to disk
//! when the user explicitly applies them.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use papr_core::{
    App, AppMode, Config, GeneralTabFocus, PathEntryState, PathsTabFocus, SettingsModalState,
    SettingsTab, Theme,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

// ---------------------------------------------------------------------------
// Public helpers called from main.rs
// ---------------------------------------------------------------------------

/// Startup page display names in the same order as `App::startup_page_options`.
const STARTUP_PAGE_LABELS: &[&str] = &[
    "Dashboard",
    "Discover",
    "Library",
    "Reading Queue",
    "Projects",
];

/// Populate the settings workspace state from the current configuration.
pub fn open_settings_modal(app: &mut App, config: &Config, current_theme_name: &str) {
    let modal = &mut app.settings_modal;

    // Remember the original theme so Esc/revert can handle it.
    modal.original_theme = current_theme_name.to_owned();

    // Theme tab
    let theme_lower = current_theme_name.to_lowercase();
    modal.theme_selected = Theme::BUILTIN_THEMES
        .iter()
        .position(|&t| t.to_lowercase() == theme_lower)
        .unwrap_or(0);
    modal.theme_scroll = 0;

    // General tab
    modal.startup_page = config.startup_page.clone();
    let sp_lower = config.startup_page.to_lowercase();
    modal.startup_page_selected = app
        .startup_page_options
        .iter()
        .position(|s| s.to_lowercase() == sp_lower)
        .unwrap_or(0);
    modal.pdf_viewer = config.pdf_viewer.clone().unwrap_or_default();
    modal.pdf_viewer_cursor = modal.pdf_viewer.len();
    modal.pdf_viewer_editing = false;
    modal.keyword_entries = config
        .dashboard_keyword_list()
        .into_iter()
        .map(PathEntryState::new)
        .collect();
    modal.keyword_selected = 0;
    modal.keyword_editing = false;
    modal.enabled_plugins = config.enabled_plugins.clone();
    modal.general_focus = GeneralTabFocus::StartupPage;

    // Paths tab
    modal.library_entries = config
        .library_folders
        .iter()
        .map(|p| PathEntryState::new(p.display().to_string()))
        .collect();
    modal.library_selected = 0;
    modal.library_editing = false;
    modal.download_path = config
        .download_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    modal.download_path_cursor = modal.download_path.len();
    modal.download_path_editing = false;
    modal.download_path_error = None;
    modal.projects_directory = config
        .projects_directory
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    modal.projects_directory_cursor = modal.projects_directory.len();
    modal.projects_directory_editing = false;
    modal.projects_directory_error = None;
    modal.paths_focus = PathsTabFocus::LibraryFolders;

    // Plugins tab
    modal.plugins_selected = 0;
    modal.plugins_scroll = 0;

    // Start on Theme tab with Tab Bar Header focused
    modal.tab = SettingsTab::Theme;
    modal.tab_bar_focused = true;

    app.mode = AppMode::Normal;
}

/// Build a [`Config`] from the current staged values in the settings state.
pub fn staged_config(modal: &SettingsModalState, options: &[String], base: &Config) -> Config {
    let startup_page = options
        .get(modal.startup_page_selected)
        .cloned()
        .unwrap_or_else(|| "dashboard".into());
    let library_folders = modal
        .library_entries
        .iter()
        .filter(|e| !e.text.trim().is_empty())
        .map(|e| PathBuf::from(&e.text))
        .collect();
    let download_path = if modal.download_path.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(&modal.download_path))
    };
    let projects_directory = if modal.projects_directory.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(&modal.projects_directory))
    };
    let pdf_viewer = if modal.pdf_viewer.trim().is_empty() {
        None
    } else {
        Some(modal.pdf_viewer.clone())
    };
    let mut keyword_list = Vec::new();
    for entry in &modal.keyword_entries {
        let trimmed = entry.text.trim();
        if !trimmed.is_empty() {
            keyword_list.push(trimmed.to_string());
        }
    }
    let dashboard_keywords = keyword_list.join(", ");

    Config {
        theme: Theme::BUILTIN_THEMES
            .get(modal.theme_selected)
            .copied()
            .unwrap_or("catppuccin-mocha")
            .to_owned(),
        startup_page,
        pdf_viewer,
        library_folders,
        download_path,
        projects_directory,
        dashboard_keywords,
        enabled_plugins: modal.enabled_plugins.clone(),
        ..base.clone()
    }
}

// ---------------------------------------------------------------------------
// Keyboard handling
// ---------------------------------------------------------------------------

/// Result of handling a key event inside the settings workspace.
pub enum SettingsKeyResult {
    /// Key was consumed; no further action needed.
    Handled,
    /// Return focus to left navigation sidebar.
    ReturnToSidebar,
    /// User pressed Apply (Enter/Save).
    Apply,
    /// User pressed q — exit application.
    Quit,
    /// Live-preview theme changed — caller must re-load theme.
    PreviewTheme(String),
}

/// Synchronize theme_selected to the index of the currently applied theme (original_theme).
pub fn sync_theme_selection_to_applied(app: &mut App) {
    let orig = app.settings_modal.original_theme.clone();
    if let Some(pos) = Theme::BUILTIN_THEMES
        .iter()
        .position(|&t| t.eq_ignore_ascii_case(&orig))
    {
        app.settings_modal.theme_selected = pos;
    }
}

/// Handle a key event while focus is inside the settings workspace.
pub fn handle_settings_key(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let is_editing = any_field_editing(app);

    // Esc key handling: if currently editing a text field, cancel text edit mode.
    // Otherwise, return focus to sidebar navigation panel!
    if key.code == KeyCode::Esc {
        if is_editing {
            clear_edit_state(app);
            return SettingsKeyResult::Handled;
        } else {
            return SettingsKeyResult::ReturnToSidebar;
        }
    }

    // Global q key handling: if not in text editing mode, quit application cleanly.
    if key.code == KeyCode::Char('q') && !is_editing {
        return SettingsKeyResult::Quit;
    }

    // Global Ctrl+B shortcut handling: toggle command palette/navigator when not in text editing mode.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('b')
        && !is_editing
    {
        app.dispatch(papr_core::Command::TogglePalette);
        return SettingsKeyResult::Handled;
    }

    // Global ? shortcut handling: toggle keyboard reference overlay when not in text editing mode.
    if is_help_shortcut(key) && !is_editing {
        app.dispatch(papr_core::Command::ToggleHelp);
        return SettingsKeyResult::Handled;
    }

    // Match the application's global Discover shortcut while leaving `/`
    // available as normal text in an actively edited settings field.
    if key.code == KeyCode::Char('/') && !is_editing {
        app.page = papr_core::Page::Discover;
        if let Some(index) = papr_core::Page::ALL
            .iter()
            .position(|&page| page == papr_core::Page::Discover)
        {
            app.sidebar_index = index;
        }
        app.content_focused = true;
        app.mode = AppMode::Search;
        app.discovery.query_cursor = app.discovery.query.len();
        return SettingsKeyResult::Handled;
    }

    // Check if current context requires Left/Right for its own control-specific functionality:
    //   • In active text edit mode: Left/Right moves text cursor.
    //   • In General tab when startup_page selector is focused: Left/Right cycles startup pages.
    let consumes_left_right = is_editing
        || (app.settings_modal.tab == SettingsTab::General
            && !app.settings_modal.tab_bar_focused
            && app.settings_modal.general_focus == GeneralTabFocus::StartupPage);

    // Universal Left / Right Arrow tab cycling across all tabs (where not consumed by control):
    if matches!(key.code, KeyCode::Left | KeyCode::Right) && !consumes_left_right {
        // Theme's left edge returns focus to the navigation panel instead of
        // wrapping back to Plugins, whether the tab bar or its list is focused.
        if key.code == KeyCode::Left && app.settings_modal.tab == SettingsTab::Theme {
            return SettingsKeyResult::ReturnToSidebar;
        }

        let prev_tab = app.settings_modal.tab;
        app.settings_modal.tab = if key.code == KeyCode::Left {
            prev_tab.prev()
        } else {
            prev_tab.next()
        };
        app.settings_modal.tab_bar_focused = true;

        if prev_tab == SettingsTab::Theme && app.settings_modal.tab != SettingsTab::Theme {
            sync_theme_selection_to_applied(app);
            return SettingsKeyResult::PreviewTheme(app.settings_modal.original_theme.clone());
        }
        if app.settings_modal.tab == SettingsTab::Theme {
            sync_theme_selection_to_applied(app);
        }
        return SettingsKeyResult::Handled;
    }

    // Tab Bar Header navigation (when tab_bar_focused is true)
    if app.settings_modal.tab_bar_focused {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                let prev_tab = app.settings_modal.tab;
                app.settings_modal.tab = prev_tab.prev();
                if app.settings_modal.tab == SettingsTab::Theme {
                    sync_theme_selection_to_applied(app);
                }
                if prev_tab == SettingsTab::Theme && app.settings_modal.tab != SettingsTab::Theme {
                    sync_theme_selection_to_applied(app);
                    return SettingsKeyResult::PreviewTheme(
                        app.settings_modal.original_theme.clone(),
                    );
                }
                return SettingsKeyResult::Handled;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let prev_tab = app.settings_modal.tab;
                app.settings_modal.tab = prev_tab.next();
                if app.settings_modal.tab == SettingsTab::Theme {
                    sync_theme_selection_to_applied(app);
                }
                if prev_tab == SettingsTab::Theme && app.settings_modal.tab != SettingsTab::Theme {
                    sync_theme_selection_to_applied(app);
                    return SettingsKeyResult::PreviewTheme(
                        app.settings_modal.original_theme.clone(),
                    );
                }
                return SettingsKeyResult::Handled;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => {
                app.settings_modal.tab_bar_focused = false;
                match app.settings_modal.tab {
                    SettingsTab::Theme => {
                        sync_theme_selection_to_applied(app);
                    }
                    SettingsTab::General => {
                        app.settings_modal.general_focus = GeneralTabFocus::StartupPage;
                    }
                    SettingsTab::Paths => {
                        app.settings_modal.paths_focus = PathsTabFocus::LibraryFolders;
                    }
                    SettingsTab::Plugins => {
                        app.settings_modal.plugins_selected = 0;
                    }
                }
                return SettingsKeyResult::Handled;
            }
            _ => return SettingsKeyResult::Handled,
        }
    }

    // Tab Body controls (when tab_bar_focused is false)
    match app.settings_modal.tab {
        SettingsTab::Theme => handle_theme_key(app, key),
        SettingsTab::General => handle_general_key(app, key),
        SettingsTab::Paths => handle_paths_key(app, key),
        SettingsTab::Plugins => handle_plugins_key(app, key),
    }
}

/// Keyboard-enhancement protocols can report `?` as the physical `/` key
/// together with `SHIFT`, instead of as the resulting character.
fn is_help_shortcut(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('?')
        || (key.code == KeyCode::Char('/') && key.modifiers.contains(KeyModifiers::SHIFT))
}

/// Text input receives the terminal's layout-resolved Unicode character.
fn text_input_char(character: char) -> char {
    character
}

fn any_field_editing(app: &App) -> bool {
    let m = &app.settings_modal;
    m.pdf_viewer_editing
        || m.keyword_editing
        || m.library_editing
        || m.download_path_editing
        || m.projects_directory_editing
}

fn clear_edit_state(app: &mut App) {
    let m = &mut app.settings_modal;
    m.pdf_viewer_editing = false;
    m.keyword_editing = false;
    m.library_editing = false;
    m.download_path_editing = false;
    m.projects_directory_editing = false;
}

// --- Theme tab ---

fn handle_theme_key(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let count = Theme::BUILTIN_THEMES.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.settings_modal.theme_selected > 0 {
                app.settings_modal.theme_selected -= 1;
                preview_selected_theme(app)
            } else {
                app.settings_modal.tab_bar_focused = true;
                SettingsKeyResult::Handled
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.settings_modal.theme_selected + 1 < count {
                app.settings_modal.theme_selected += 1;
                preview_selected_theme(app)
            } else {
                SettingsKeyResult::Handled
            }
        }
        KeyCode::Left => {
            let prev_tab = app.settings_modal.tab;
            app.settings_modal.tab = prev_tab.prev();
            app.settings_modal.tab_bar_focused = true;
            SettingsKeyResult::PreviewTheme(app.settings_modal.original_theme.clone())
        }
        KeyCode::Right => {
            let prev_tab = app.settings_modal.tab;
            app.settings_modal.tab = prev_tab.next();
            app.settings_modal.tab_bar_focused = true;
            SettingsKeyResult::PreviewTheme(app.settings_modal.original_theme.clone())
        }
        KeyCode::Enter => SettingsKeyResult::Apply,
        _ => SettingsKeyResult::Handled,
    }
}

fn preview_selected_theme(app: &App) -> SettingsKeyResult {
    let name = Theme::BUILTIN_THEMES
        .get(app.settings_modal.theme_selected)
        .copied()
        .unwrap_or("catppuccin-mocha");
    SettingsKeyResult::PreviewTheme(name.to_owned())
}

// --- General tab ---

fn handle_general_key(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let focus = app.settings_modal.general_focus;

    if app.settings_modal.pdf_viewer_editing {
        return handle_pdf_viewer_edit(app, key);
    }
    if app.settings_modal.keyword_editing {
        return handle_keyword_entry_edit(app, key);
    }

    match focus {
        GeneralTabFocus::StartupPage => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_modal.tab_bar_focused = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let opts_len = app.startup_page_options.len();
                if opts_len > 0 {
                    app.settings_modal.startup_page_selected =
                        (app.settings_modal.startup_page_selected + opts_len - 1) % opts_len;
                    app.settings_modal.startup_page = app
                        .startup_page_options
                        .get(app.settings_modal.startup_page_selected)
                        .cloned()
                        .unwrap_or_default();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let opts_len = app.startup_page_options.len();
                if opts_len > 0 {
                    app.settings_modal.startup_page_selected =
                        (app.settings_modal.startup_page_selected + 1) % opts_len;
                    app.settings_modal.startup_page = app
                        .startup_page_options
                        .get(app.settings_modal.startup_page_selected)
                        .cloned()
                        .unwrap_or_default();
                }
            }
            KeyCode::Enter => return SettingsKeyResult::Apply,
            _ => {}
        },

        GeneralTabFocus::PdfViewer => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_modal.general_focus = GeneralTabFocus::StartupPage;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.settings_modal.general_focus = GeneralTabFocus::DashboardKeywords;
                if !app.settings_modal.keyword_entries.is_empty() {
                    app.settings_modal.keyword_selected = 0;
                }
            }
            KeyCode::Enter => {
                app.settings_modal.pdf_viewer_editing = true;
            }
            _ => {}
        },

        GeneralTabFocus::DashboardKeywords => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if app.settings_modal.keyword_selected > 0 {
                    app.settings_modal.keyword_selected -= 1;
                } else {
                    app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = app.settings_modal.keyword_entries.len();
                if len > 0 && app.settings_modal.keyword_selected + 1 < len {
                    app.settings_modal.keyword_selected += 1;
                } else {
                    app.settings_modal.general_focus = GeneralTabFocus::EnabledPlugins;
                }
            }
            KeyCode::Enter => {
                if !app.settings_modal.keyword_entries.is_empty() {
                    app.settings_modal.keyword_editing = true;
                }
            }
            KeyCode::Char('a') => {
                app.settings_modal
                    .keyword_entries
                    .push(PathEntryState::new(String::new()));
                app.settings_modal.keyword_selected = app.settings_modal.keyword_entries.len() - 1;
                app.settings_modal.keyword_editing = true;
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let idx = app.settings_modal.keyword_selected;
                if !app.settings_modal.keyword_entries.is_empty() {
                    app.settings_modal.keyword_entries.remove(idx);
                    if app.settings_modal.keyword_selected
                        >= app.settings_modal.keyword_entries.len()
                        && app.settings_modal.keyword_selected > 0
                    {
                        app.settings_modal.keyword_selected -= 1;
                    }
                    return SettingsKeyResult::Apply;
                }
            }
            KeyCode::Char('K') => {
                let idx = app.settings_modal.keyword_selected;
                if idx > 0 {
                    app.settings_modal.keyword_entries.swap(idx, idx - 1);
                    app.settings_modal.keyword_selected -= 1;
                    return SettingsKeyResult::Apply;
                }
            }
            KeyCode::Char('J') => {
                let idx = app.settings_modal.keyword_selected;
                let len = app.settings_modal.keyword_entries.len();
                if len > 0 && idx + 1 < len {
                    app.settings_modal.keyword_entries.swap(idx, idx + 1);
                    app.settings_modal.keyword_selected += 1;
                    return SettingsKeyResult::Apply;
                }
            }
            _ => {}
        },

        GeneralTabFocus::EnabledPlugins => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_modal.general_focus = GeneralTabFocus::DashboardKeywords;
                if !app.settings_modal.keyword_entries.is_empty() {
                    app.settings_modal.keyword_selected =
                        app.settings_modal.keyword_entries.len() - 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {}
            KeyCode::Char(' ') => {
                if let Some(plugin) = app.plugins.get(app.settings_modal.plugins_selected) {
                    let id = plugin.id.clone();
                    if app.settings_modal.enabled_plugins.contains(&id) {
                        app.settings_modal.enabled_plugins.retain(|p| p != &id);
                    } else {
                        app.settings_modal.enabled_plugins.push(id);
                    }
                    return SettingsKeyResult::Apply;
                }
            }
            _ => {}
        },
    }
    SettingsKeyResult::Handled
}

fn handle_pdf_viewer_edit(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    match key.code {
        KeyCode::Enter => {
            app.settings_modal.pdf_viewer_editing = false;
            return SettingsKeyResult::Apply;
        }
        KeyCode::Esc => {
            app.settings_modal.pdf_viewer_editing = false;
            return SettingsKeyResult::Handled;
        }
        KeyCode::Char(c) => {
            let c = text_input_char(c);
            let cur = app.settings_modal.pdf_viewer_cursor;
            app.settings_modal.pdf_viewer.insert(cur, c);
            app.settings_modal.pdf_viewer_cursor = cur + c.len_utf8();
        }
        KeyCode::Backspace => {
            let cur = app.settings_modal.pdf_viewer_cursor;
            if cur > 0 {
                let prev = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    prev_word_boundary(&app.settings_modal.pdf_viewer, cur)
                } else {
                    prev_char_boundary(&app.settings_modal.pdf_viewer, cur)
                };
                app.settings_modal.pdf_viewer.drain(prev..cur);
                app.settings_modal.pdf_viewer_cursor = prev;
            }
        }
        KeyCode::Delete => {
            let cur = app.settings_modal.pdf_viewer_cursor;
            if cur < app.settings_modal.pdf_viewer.len() {
                let next = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    next_word_boundary(&app.settings_modal.pdf_viewer, cur)
                } else {
                    next_char_boundary(&app.settings_modal.pdf_viewer, cur)
                };
                app.settings_modal.pdf_viewer.drain(cur..next);
            }
        }
        KeyCode::Left => {
            let cur = app.settings_modal.pdf_viewer_cursor;
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.settings_modal.pdf_viewer_cursor =
                    prev_word_boundary(&app.settings_modal.pdf_viewer, cur);
            } else if cur > 0 {
                app.settings_modal.pdf_viewer_cursor =
                    prev_char_boundary(&app.settings_modal.pdf_viewer, cur);
            }
        }
        KeyCode::Right => {
            let cur = app.settings_modal.pdf_viewer_cursor;
            let len = app.settings_modal.pdf_viewer.len();
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.settings_modal.pdf_viewer_cursor =
                    next_word_boundary(&app.settings_modal.pdf_viewer, cur);
            } else if cur < len {
                app.settings_modal.pdf_viewer_cursor =
                    next_char_boundary(&app.settings_modal.pdf_viewer, cur);
            }
        }
        KeyCode::Home => {
            app.settings_modal.pdf_viewer_cursor = 0;
        }
        KeyCode::End => {
            app.settings_modal.pdf_viewer_cursor = app.settings_modal.pdf_viewer.len();
        }
        _ => {}
    }
    SettingsKeyResult::Handled
}

// --- Paths tab ---

fn handle_paths_key(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let focus = app.settings_modal.paths_focus;

    // Route active text edits
    if app.settings_modal.library_editing && focus == PathsTabFocus::LibraryFolders {
        return handle_library_entry_edit(app, key);
    }
    if app.settings_modal.download_path_editing && focus == PathsTabFocus::DownloadPath {
        return handle_single_path_edit(app, PathsTabFocus::DownloadPath, key);
    }
    if app.settings_modal.projects_directory_editing && focus == PathsTabFocus::ProjectsDirectory {
        return handle_single_path_edit(app, PathsTabFocus::ProjectsDirectory, key);
    }

    match focus {
        PathsTabFocus::LibraryFolders => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if app.settings_modal.library_selected > 0 {
                    app.settings_modal.library_selected -= 1;
                } else {
                    app.settings_modal.tab_bar_focused = true;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = app.settings_modal.library_entries.len();
                if len > 0 && app.settings_modal.library_selected + 1 < len {
                    app.settings_modal.library_selected += 1;
                } else {
                    app.settings_modal.paths_focus = PathsTabFocus::DownloadPath;
                }
            }
            KeyCode::Enter => {
                if !app.settings_modal.library_entries.is_empty() {
                    app.settings_modal.library_editing = true;
                }
            }
            KeyCode::Char('a') => {
                app.settings_modal
                    .library_entries
                    .push(PathEntryState::new(String::new()));
                app.settings_modal.library_selected = app.settings_modal.library_entries.len() - 1;
                app.settings_modal.library_editing = true;
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let idx = app.settings_modal.library_selected;
                if !app.settings_modal.library_entries.is_empty() {
                    app.settings_modal.library_entries.remove(idx);
                    if app.settings_modal.library_selected
                        >= app.settings_modal.library_entries.len()
                        && app.settings_modal.library_selected > 0
                    {
                        app.settings_modal.library_selected -= 1;
                    }
                    return SettingsKeyResult::Apply;
                }
            }
            KeyCode::Char('K') => {
                let idx = app.settings_modal.library_selected;
                if idx > 0 {
                    app.settings_modal.library_entries.swap(idx, idx - 1);
                    app.settings_modal.library_selected -= 1;
                    return SettingsKeyResult::Apply;
                }
            }
            KeyCode::Char('J') => {
                let idx = app.settings_modal.library_selected;
                let len = app.settings_modal.library_entries.len();
                if len > 0 && idx + 1 < len {
                    app.settings_modal.library_entries.swap(idx, idx + 1);
                    app.settings_modal.library_selected += 1;
                    return SettingsKeyResult::Apply;
                }
            }
            KeyCode::Tab => {
                app.settings_modal.paths_focus = PathsTabFocus::DownloadPath;
            }
            KeyCode::BackTab => {
                app.settings_modal.tab_bar_focused = true;
            }
            _ => {}
        },

        PathsTabFocus::DownloadPath => match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                app.settings_modal.paths_focus = PathsTabFocus::LibraryFolders;
                if !app.settings_modal.library_entries.is_empty() {
                    app.settings_modal.library_selected =
                        app.settings_modal.library_entries.len() - 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                app.settings_modal.paths_focus = PathsTabFocus::ProjectsDirectory;
            }
            KeyCode::Enter => {
                app.settings_modal.download_path_editing = true;
            }
            _ => {}
        },

        PathsTabFocus::ProjectsDirectory => match key.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                app.settings_modal.paths_focus = PathsTabFocus::DownloadPath;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {}
            KeyCode::Enter => {
                app.settings_modal.projects_directory_editing = true;
            }
            _ => {}
        },
    }
    SettingsKeyResult::Handled
}

fn handle_library_entry_edit(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let idx = app.settings_modal.library_selected;
    match key.code {
        KeyCode::Enter => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let err = validate_path(&entry.text);
                entry.error = err.clone();
                app.settings_modal.library_editing = false;
                if err.is_none() {
                    return SettingsKeyResult::Apply;
                }
            } else {
                app.settings_modal.library_editing = false;
            }
        }
        KeyCode::Esc => {
            app.settings_modal.library_editing = false;
        }
        KeyCode::Char(c) => {
            let c = text_input_char(c);
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let cur = entry.cursor;
                entry.text.insert(cur, c);
                entry.cursor = cur + c.len_utf8();
                entry.error = None;
            }
        }
        KeyCode::Backspace => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let cur = entry.cursor;
                if cur > 0 {
                    let prev = if key.modifiers.contains(KeyModifiers::CONTROL) {
                        prev_word_boundary(&entry.text, cur)
                    } else {
                        prev_char_boundary(&entry.text, cur)
                    };
                    entry.text.drain(prev..cur);
                    entry.cursor = prev;
                    entry.error = None;
                }
            }
        }
        KeyCode::Delete => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let cur = entry.cursor;
                if cur < entry.text.len() {
                    let next = if key.modifiers.contains(KeyModifiers::CONTROL) {
                        next_word_boundary(&entry.text, cur)
                    } else {
                        next_char_boundary(&entry.text, cur)
                    };
                    entry.text.drain(cur..next);
                    entry.error = None;
                }
            }
        }
        KeyCode::Left => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let cur = entry.cursor;
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    entry.cursor = prev_word_boundary(&entry.text, cur);
                } else if cur > 0 {
                    entry.cursor = prev_char_boundary(&entry.text, cur);
                }
            }
        }
        KeyCode::Right => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                let cur = entry.cursor;
                let len = entry.text.len();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    entry.cursor = next_word_boundary(&entry.text, cur);
                } else if cur < len {
                    entry.cursor = next_char_boundary(&entry.text, cur);
                }
            }
        }
        KeyCode::Home => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                entry.cursor = 0;
            }
        }
        KeyCode::End => {
            if let Some(entry) = app.settings_modal.library_entries.get_mut(idx) {
                entry.cursor = entry.text.len();
            }
        }
        _ => {}
    }
    SettingsKeyResult::Handled
}

fn handle_single_path_edit(
    app: &mut App,
    focus: PathsTabFocus,
    key: KeyEvent,
) -> SettingsKeyResult {
    match key.code {
        KeyCode::Enter => match focus {
            PathsTabFocus::DownloadPath => {
                let err = validate_path(&app.settings_modal.download_path);
                app.settings_modal.download_path_error = err.clone();
                app.settings_modal.download_path_editing = false;
                if err.is_none() {
                    return SettingsKeyResult::Apply;
                }
            }
            PathsTabFocus::ProjectsDirectory => {
                let err = validate_path(&app.settings_modal.projects_directory);
                app.settings_modal.projects_directory_error = err.clone();
                app.settings_modal.projects_directory_editing = false;
                if err.is_none() {
                    return SettingsKeyResult::Apply;
                }
            }
            _ => {}
        },
        KeyCode::Esc => match focus {
            PathsTabFocus::DownloadPath => {
                app.settings_modal.download_path_editing = false;
            }
            PathsTabFocus::ProjectsDirectory => {
                app.settings_modal.projects_directory_editing = false;
            }
            _ => {}
        },
        KeyCode::Char(c) => {
            let c = text_input_char(c);
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            let cur = *cursor;
            text.insert(cur, c);
            *cursor = cur + c.len_utf8();
        }
        KeyCode::Backspace => {
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            let cur = *cursor;
            if cur > 0 {
                let prev = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    prev_word_boundary(text, cur)
                } else {
                    prev_char_boundary(text, cur)
                };
                text.drain(prev..cur);
                *cursor = prev;
            }
        }
        KeyCode::Delete => {
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            let cur = *cursor;
            if cur < text.len() {
                let next = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    next_word_boundary(text, cur)
                } else {
                    next_char_boundary(text, cur)
                };
                text.drain(cur..next);
            }
        }
        KeyCode::Left => {
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            let cur = *cursor;
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                *cursor = prev_word_boundary(text, cur);
            } else if cur > 0 {
                *cursor = prev_char_boundary(text, cur);
            }
        }
        KeyCode::Right => {
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            let cur = *cursor;
            let len = text.len();
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                *cursor = next_word_boundary(text, cur);
            } else if cur < len {
                *cursor = next_char_boundary(text, cur);
            }
        }
        KeyCode::Home => {
            let cursor = match focus {
                PathsTabFocus::DownloadPath => &mut app.settings_modal.download_path_cursor,
                PathsTabFocus::ProjectsDirectory => {
                    &mut app.settings_modal.projects_directory_cursor
                }
                _ => return SettingsKeyResult::Handled,
            };
            *cursor = 0;
        }
        KeyCode::End => {
            let (text, cursor) = match focus {
                PathsTabFocus::DownloadPath => (
                    &mut app.settings_modal.download_path,
                    &mut app.settings_modal.download_path_cursor,
                ),
                PathsTabFocus::ProjectsDirectory => (
                    &mut app.settings_modal.projects_directory,
                    &mut app.settings_modal.projects_directory_cursor,
                ),
                _ => return SettingsKeyResult::Handled,
            };
            *cursor = text.len();
        }
        _ => {}
    }
    SettingsKeyResult::Handled
}

// --- Plugins tab ---

fn handle_plugins_key(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let count = app.plugins.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.settings_modal.plugins_selected > 0 {
                app.settings_modal.plugins_selected -= 1;
            } else {
                app.settings_modal.tab_bar_focused = true;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 && app.settings_modal.plugins_selected + 1 < count {
                app.settings_modal.plugins_selected += 1;
            }
        }
        KeyCode::Char(' ') => {
            let idx = app.settings_modal.plugins_selected;
            if let Some(plugin) = app.plugins.get(idx) {
                let id = plugin.id.clone();
                if app.settings_modal.enabled_plugins.contains(&id) {
                    app.settings_modal.enabled_plugins.retain(|p| p != &id);
                } else {
                    app.settings_modal.enabled_plugins.push(id);
                }
            }
        }
        KeyCode::Enter => {
            return SettingsKeyResult::Apply;
        }
        _ => {}
    }
    SettingsKeyResult::Handled
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the embedded settings workspace UI inside `area`.
#[allow(dead_code)]
pub fn render_settings_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    render_settings_ui(frame, area, app, theme);
}

/// Render the embedded settings workspace UI inside `area`.
pub fn render_settings_ui(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let outer_block = Block::default()
        .title(Line::styled(
            " SETTINGS ",
            if app.content_focused {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_set(border::ROUNDED)
        .border_style(if app.content_focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.border)
        })
        .style(Style::default().bg(theme.surface));

    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(2),
    ])
    .split(inner);

    render_tab_bar(frame, sections[0], app, theme);
    render_tab_content(frame, sections[1], app, theme);
    render_footer(frame, sections[2], app, theme);
}

fn render_tab_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let is_tab_bar_active = app.content_focused && app.settings_modal.tab_bar_focused;

    let tabs: Vec<Span> = SettingsTab::ALL
        .iter()
        .flat_map(|&tab| {
            let active = tab == app.settings_modal.tab;
            let style = if active {
                if is_tab_bar_active {
                    Style::default()
                        .fg(theme.background)
                        .bg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else if app.content_focused {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
                }
            } else {
                Style::default().fg(theme.muted)
            };
            let label = format!(" {} ", tab.title());
            vec![Span::styled(label, style), Span::raw("  ")]
        })
        .collect();

    let tab_line = Paragraph::new(Line::from(tabs));
    frame.render_widget(tab_line, area);

    if area.width > 40 {
        let hint_text = if is_tab_bar_active {
            "◄/► Switch Tab  ▼ Focus Settings"
        } else {
            "◄/► Switch Tab"
        };
        let hint_len = hint_text.chars().count() as u16;
        let hint_area = Rect::new(
            area.x + area.width.saturating_sub(hint_len),
            area.y,
            hint_len,
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(hint_text, Style::default().fg(theme.muted)))
                .alignment(Alignment::Right),
            hint_area,
        );
    }
}

fn render_tab_content(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let divider_area = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.border),
        )),
        divider_area,
    );
    let content_area = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    match app.settings_modal.tab {
        SettingsTab::Theme => render_theme_tab(frame, content_area, app, theme),
        SettingsTab::General => render_general_tab(frame, content_area, app, theme),
        SettingsTab::Paths => render_paths_tab(frame, content_area, app, theme),
        SettingsTab::Plugins => render_plugins_tab(frame, content_area, app, theme),
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let divider = Paragraph::new(Line::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(theme.border),
    ));
    let div_area = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(divider, div_area);

    let footer_area = Rect::new(area.x, area.y + 1, area.width, 1);
    let is_editing = any_field_editing(app);

    let keys = if !app.content_focused {
        vec![
            Span::styled(
                "Right / Enter",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " Focus Settings Workspace",
                Style::default().fg(theme.muted),
            ),
        ]
    } else if is_editing {
        vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Commit Edit  ", Style::default().fg(theme.text)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel Edit", Style::default().fg(theme.text)),
        ]
    } else if app.settings_modal.tab_bar_focused {
        vec![
            Span::styled(
                "◄ / ►",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Switch Tab  ", Style::default().fg(theme.text)),
            Span::styled(
                "▼ / Enter",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Focus Tab Content  ", Style::default().fg(theme.text)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Return to Navigation", Style::default().fg(theme.muted)),
        ]
    } else {
        vec![
            Span::styled(
                "▲ / ▼",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Navigate Controls  ", Style::default().fg(theme.text)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Edit / Apply  ", Style::default().fg(theme.text)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Return to Navigation", Style::default().fg(theme.muted)),
        ]
    };
    frame.render_widget(Paragraph::new(Line::from(keys)), footer_area);
}

// --- Theme tab rendering ---

fn render_theme_tab(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);

    let is_focused = app.content_focused && !app.settings_modal.tab_bar_focused;

    frame.render_widget(
        Paragraph::new(Line::styled(
            "  ↑/↓ to navigate. Selection previews immediately. Enter to Apply.",
            Style::default().fg(theme.muted),
        )),
        chunks[0],
    );

    let list_area = chunks[1];
    let viewport_h = list_area.height as usize;

    if app.settings_modal.theme_selected < app.settings_modal.theme_scroll {
        app.settings_modal.theme_scroll = app.settings_modal.theme_selected;
    } else if app.settings_modal.theme_selected
        >= app.settings_modal.theme_scroll + viewport_h.max(1)
    {
        app.settings_modal.theme_scroll = app.settings_modal.theme_selected + 1 - viewport_h.max(1);
    }

    let items: Vec<ListItem> = Theme::BUILTIN_THEMES
        .iter()
        .enumerate()
        .map(|(i, &name)| {
            let is_cursor = i == app.settings_modal.theme_selected;
            let is_applied = name.eq_ignore_ascii_case(&app.settings_modal.original_theme);

            let mut style = if is_cursor && is_focused {
                Style::default().fg(theme.text).bg(theme.accent)
            } else {
                Style::default().fg(theme.muted)
            };

            if is_applied {
                style = style.add_modifier(Modifier::BOLD);
            }

            let display = name
                .split('-')
                .map(|s| {
                    let mut c = s.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");

            let prefix = if is_cursor { "> " } else { "  " };
            let suffix = if is_applied { " (Active)" } else { "" };
            let label = format!("{}{}{}", prefix, display, suffix);

            ListItem::new(Line::styled(label, style))
        })
        .collect();

    let mut list_state = ListState::default()
        .with_selected(Some(app.settings_modal.theme_selected))
        .with_offset(app.settings_modal.theme_scroll);

    frame.render_stateful_widget(
        List::new(items).block(focused_block(" Available Themes ", is_focused, theme)),
        list_area,
        &mut list_state,
    );
    app.settings_modal.theme_scroll = list_state.offset();
}

fn handle_keyword_entry_edit(app: &mut App, key: KeyEvent) -> SettingsKeyResult {
    let idx = app.settings_modal.keyword_selected;
    match key.code {
        KeyCode::Enter => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let trimmed = entry.text.trim();
                if trimmed.is_empty() {
                    entry.error = Some("Keyword cannot be empty".to_string());
                    return SettingsKeyResult::Handled;
                }
                entry.text = trimmed.to_string();
                entry.error = None;
                app.settings_modal.keyword_editing = false;
                return SettingsKeyResult::Apply;
            } else {
                app.settings_modal.keyword_editing = false;
            }
        }
        KeyCode::Esc => {
            app.settings_modal.keyword_editing = false;
        }
        KeyCode::Char(c) => {
            let c = text_input_char(c);
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let cur = entry.cursor;
                entry.text.insert(cur, c);
                entry.cursor = cur + c.len_utf8();
                entry.error = None;
            }
        }
        KeyCode::Backspace => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let cur = entry.cursor;
                if cur > 0 {
                    let prev = if key.modifiers.contains(KeyModifiers::CONTROL) {
                        prev_word_boundary(&entry.text, cur)
                    } else {
                        prev_char_boundary(&entry.text, cur)
                    };
                    entry.text.drain(prev..cur);
                    entry.cursor = prev;
                    entry.error = None;
                }
            }
        }
        KeyCode::Delete => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let cur = entry.cursor;
                if cur < entry.text.len() {
                    let next = if key.modifiers.contains(KeyModifiers::CONTROL) {
                        next_word_boundary(&entry.text, cur)
                    } else {
                        next_char_boundary(&entry.text, cur)
                    };
                    entry.text.drain(cur..next);
                    entry.error = None;
                }
            }
        }
        KeyCode::Left => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let cur = entry.cursor;
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    entry.cursor = prev_word_boundary(&entry.text, cur);
                } else if cur > 0 {
                    entry.cursor = prev_char_boundary(&entry.text, cur);
                }
            }
        }
        KeyCode::Right => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                let cur = entry.cursor;
                let len = entry.text.len();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    entry.cursor = next_word_boundary(&entry.text, cur);
                } else if cur < len {
                    entry.cursor = next_char_boundary(&entry.text, cur);
                }
            }
        }
        KeyCode::Home => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                entry.cursor = 0;
            }
        }
        KeyCode::End => {
            if let Some(entry) = app.settings_modal.keyword_entries.get_mut(idx) {
                entry.cursor = entry.text.len();
            }
        }
        _ => {}
    }
    SettingsKeyResult::Handled
}

// --- General tab rendering ---

fn render_general_tab(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let kw_count = app.settings_modal.keyword_entries.len();
    let kw_height = (kw_count as u16 + 2)
        .max(4)
        .min(area.height.saturating_sub(10));

    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(kw_height),
        Constraint::Min(3),
    ])
    .margin(1)
    .split(area);

    let is_body_active = app.content_focused && !app.settings_modal.tab_bar_focused;

    // startup_page
    let startup_focused =
        is_body_active && app.settings_modal.general_focus == GeneralTabFocus::StartupPage;
    let startup_block = focused_block(" Startup Page (◄/► to cycle) ", startup_focused, theme);
    let current_label = STARTUP_PAGE_LABELS
        .get(app.settings_modal.startup_page_selected)
        .copied()
        .unwrap_or("Dashboard");
    let total = STARTUP_PAGE_LABELS.len();
    let startup_text = format!(
        " ◄  {}  ►   ({}/{}) ",
        current_label,
        app.settings_modal.startup_page_selected + 1,
        total
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            startup_text,
            Style::default()
                .fg(if startup_focused { theme.text } else { theme.muted })
                .add_modifier(if startup_focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
        .block(startup_block),
        chunks[0],
    );

    // pdf_viewer
    let viewer_focused =
        is_body_active && app.settings_modal.general_focus == GeneralTabFocus::PdfViewer;
    let viewer_editing = app.settings_modal.pdf_viewer_editing;
    let viewer_title = if viewer_editing {
        " PDF Viewer Command [editing — Enter commit, Esc cancel] "
    } else {
        " PDF Viewer Command (Enter to edit) "
    };
    let viewer_block = focused_block(viewer_title, viewer_focused, theme);
    let viewer_style = if viewer_editing {
        Style::default().fg(theme.text)
    } else {
        setting_value_style(viewer_focused, theme)
    };
    frame.render_widget(
        Paragraph::new(Line::styled(&app.settings_modal.pdf_viewer, viewer_style))
            .block(viewer_block),
        chunks[1],
    );

    if viewer_editing {
        let visible_cols = app.settings_modal.pdf_viewer[..app.settings_modal.pdf_viewer_cursor]
            .chars()
            .count() as u16;
        frame.set_cursor_position((chunks[1].x + 1 + visible_cols, chunks[1].y + 1));
    }

    // dashboard_keywords
    let keywords_focused =
        is_body_active && app.settings_modal.general_focus == GeneralTabFocus::DashboardKeywords;
    render_dashboard_keywords(frame, chunks[2], app, keywords_focused, theme);

    // enabled_plugins
    let plugins_focused =
        is_body_active && app.settings_modal.general_focus == GeneralTabFocus::EnabledPlugins;
    let enabled_count = app.settings_modal.enabled_plugins.len();
    let plugin_hint = if app.plugins.is_empty() {
        "No plugins installed.".to_owned()
    } else {
        format!(
            "{}/{} plugins enabled — go to Plugins tab to manage",
            enabled_count,
            app.plugins.len()
        )
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            plugin_hint,
            setting_value_style(plugins_focused, theme),
        ))
        .block(focused_block(
            " Plugins (see Plugins tab) ",
            plugins_focused,
            theme,
        )),
        chunks[3],
    );
}

fn render_dashboard_keywords(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    focused: bool,
    theme: &Theme,
) {
    let editing = app.settings_modal.keyword_editing;
    let title = if editing {
        " Dashboard Search Keywords [editing — Enter commit, Esc cancel] "
    } else {
        " Dashboard Search Keywords  a Add  d Remove  K/J Reorder  Enter Edit "
    };
    let block = focused_block(title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sel = app.settings_modal.keyword_selected;
    let entries = &app.settings_modal.keyword_entries;

    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  No keywords configured. Press a to add one.",
                Style::default().fg(theme.muted),
            )),
            inner,
        );
        return;
    }

    let wrap_width = inner.width.saturating_sub(2).max(1) as usize;
    let mut y_offset = 0u16;

    for (i, entry) in entries.iter().enumerate() {
        if y_offset >= inner.height {
            break;
        }
        let is_selected = i == sel && focused;
        let is_editing = editing && is_selected;

        let text_rows = entry.text.len().div_ceil(wrap_width.max(1)).max(1) as u16;
        let error_rows = if entry.error.is_some() { 1u16 } else { 0 };
        let total_rows = text_rows + error_rows;

        let row_area = Rect::new(
            inner.x,
            inner.y + y_offset,
            inner.width,
            total_rows.min(inner.height - y_offset),
        );

        let base_style = setting_value_style(is_selected, theme).add_modifier(if is_selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        let prefix = if is_selected { "› " } else { "  " };

        let display_text = format!("{}{}", prefix, entry.text);
        let paragraph = Paragraph::new(display_text)
            .style(base_style)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, row_area);

        if let Some(ref err) = entry.error {
            let err_area = Rect::new(
                row_area.x + 2,
                row_area.y + text_rows,
                row_area.width.saturating_sub(2),
                1,
            );
            frame.render_widget(
                Paragraph::new(format!("⚠ {err}")).style(Style::default().fg(theme.warning)),
                err_area,
            );
        }

        if is_editing {
            let cur = entry.text[..entry.cursor].chars().count() as u16;
            let cursor_x = row_area.x + 2 + (cur % (wrap_width as u16));
            let cursor_y = row_area.y + (cur / (wrap_width as u16));
            if cursor_y < inner.y + inner.height {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }

        y_offset += total_rows;
    }
}

// --- Paths tab rendering ---

fn render_paths_tab(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let chunks = Layout::vertical([
        Constraint::Percentage(50),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .margin(1)
    .split(area);

    let is_body_active = app.content_focused && !app.settings_modal.tab_bar_focused;

    let library_focused =
        is_body_active && app.settings_modal.paths_focus == PathsTabFocus::LibraryFolders;
    let download_focused =
        is_body_active && app.settings_modal.paths_focus == PathsTabFocus::DownloadPath;
    let projects_focused =
        is_body_active && app.settings_modal.paths_focus == PathsTabFocus::ProjectsDirectory;

    render_library_folders(frame, chunks[0], app, library_focused, theme);

    render_single_path_field(
        frame,
        chunks[1],
        " Download Path (Enter to edit) ",
        &app.settings_modal.download_path,
        app.settings_modal.download_path_cursor,
        app.settings_modal.download_path_editing,
        download_focused,
        app.settings_modal.download_path_error.as_deref(),
        theme,
    );

    render_single_path_field(
        frame,
        chunks[2],
        " Projects Directory (Enter to edit) ",
        &app.settings_modal.projects_directory,
        app.settings_modal.projects_directory_cursor,
        app.settings_modal.projects_directory_editing,
        projects_focused,
        app.settings_modal.projects_directory_error.as_deref(),
        theme,
    );

    if app.settings_modal.download_path_editing && download_focused {
        let cur = app.settings_modal.download_path[..app.settings_modal.download_path_cursor]
            .chars()
            .count() as u16;
        frame.set_cursor_position((chunks[1].x + 1 + cur, chunks[1].y + 1));
    } else if app.settings_modal.projects_directory_editing && projects_focused {
        let cur = app.settings_modal.projects_directory
            [..app.settings_modal.projects_directory_cursor]
            .chars()
            .count() as u16;
        frame.set_cursor_position((chunks[2].x + 1 + cur, chunks[2].y + 1));
    }
}

fn render_library_folders(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    focused: bool,
    theme: &Theme,
) {
    let editing = app.settings_modal.library_editing;
    let title = if editing {
        " Library Folders [editing — Enter commit, Esc cancel] "
    } else {
        " Library Folders  a Add  d Remove  K/J Reorder  Enter Edit "
    };
    let block = focused_block(title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sel = app.settings_modal.library_selected;
    let entries = &app.settings_modal.library_entries;

    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  No library folders. Press a to add one.",
                Style::default().fg(theme.muted),
            )),
            inner,
        );
        return;
    }

    let wrap_width = inner.width.saturating_sub(2).max(1) as usize;
    let mut y_offset = 0u16;

    for (i, entry) in entries.iter().enumerate() {
        if y_offset >= inner.height {
            break;
        }
        let is_selected = i == sel && focused;
        let is_editing = editing && is_selected;

        let text_rows = entry.text.len().div_ceil(wrap_width.max(1)).max(1) as u16;
        let error_rows = if entry.error.is_some() { 1u16 } else { 0 };
        let total_rows = text_rows + error_rows;

        let row_area = Rect::new(
            inner.x,
            inner.y + y_offset,
            inner.width,
            total_rows.min(inner.height - y_offset),
        );

        let base_style = setting_value_style(is_selected, theme).add_modifier(if is_selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        let prefix = if is_selected { "› " } else { "  " };
        let display_text = format!("{}{}", prefix, entry.text);

        if is_editing {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    &display_text,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ))
                .wrap(Wrap { trim: false }),
                row_area,
            );
            let before_cursor = format!("{}{}", prefix, &entry.text[..entry.cursor]);
            let col = before_cursor.chars().count() as u16;
            let cursor_row = col / inner.width.max(1);
            let cursor_col = col % inner.width.max(1);
            if inner.y + y_offset + cursor_row < inner.y + inner.height {
                frame.set_cursor_position((inner.x + cursor_col, inner.y + y_offset + cursor_row));
            }
        } else {
            frame.render_widget(
                Paragraph::new(Line::styled(&display_text, base_style)).wrap(Wrap { trim: false }),
                row_area,
            );
        }

        if let Some(ref err) = entry.error {
            let err_y = y_offset + text_rows;
            if err_y < inner.height {
                let err_area = Rect::new(inner.x, inner.y + err_y, inner.width, 1);
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        format!("  ⚠ {}", err),
                        Style::default().fg(theme.error),
                    )),
                    err_area,
                );
            }
        }

        y_offset += total_rows;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_single_path_field(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    text: &str,
    _cursor: usize,
    editing: bool,
    focused: bool,
    error: Option<&str>,
    theme: &Theme,
) {
    let effective_title = if error.is_some() {
        format!("{} ⚠", title.trim_end())
    } else {
        title.to_owned()
    };
    let block = focused_block(&effective_title, focused, theme);
    let style = if editing {
        Style::default().fg(theme.text)
    } else if error.is_some() {
        Style::default().fg(theme.error)
    } else {
        setting_value_style(focused, theme)
    };
    let content = if let Some(err) = error {
        format!("{} — {}", text, err)
    } else {
        text.to_owned()
    };
    frame.render_widget(
        Paragraph::new(Line::styled(content, style)).block(block),
        area,
    );
}

// --- Plugins tab rendering ---

fn render_plugins_tab(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.content_focused && !app.settings_modal.tab_bar_focused;

    if app.plugins.is_empty() {
        let block = focused_block(" Plugins ", is_focused, theme);
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  No plugins installed.",
                Style::default().fg(theme.muted),
            ))
            .block(block),
            area,
        );
        return;
    }

    let enabled_ids = &app.settings_modal.enabled_plugins;
    let items: Vec<ListItem> = app
        .plugins
        .iter()
        .enumerate()
        .map(|(i, plugin)| {
            let selected = i == app.settings_modal.plugins_selected && is_focused;
            let staged_enabled = enabled_ids.contains(&plugin.id);
            let check = if staged_enabled { "✓" } else { "✗" };
            let check_style = setting_value_style(selected, theme);
            let name_style = setting_value_style(selected, theme).add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
            let detail_style = setting_value_style(selected, theme);
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("[{}] ", check), check_style),
                Span::styled(format!("{} ", plugin.name), name_style),
                Span::styled(
                    format!("v{}", plugin.version),
                    detail_style,
                ),
                Span::raw("  "),
                Span::styled(&plugin.description, detail_style),
            ]))
        })
        .collect();

    let viewport_h = area.height.saturating_sub(2) as usize;
    if app.settings_modal.plugins_selected < app.settings_modal.plugins_scroll {
        app.settings_modal.plugins_scroll = app.settings_modal.plugins_selected;
    } else if app.settings_modal.plugins_selected
        >= app.settings_modal.plugins_scroll + viewport_h.max(1)
    {
        app.settings_modal.plugins_scroll =
            app.settings_modal.plugins_selected + 1 - viewport_h.max(1);
    }

    let mut state = ListState::default()
        .with_selected(Some(app.settings_modal.plugins_selected))
        .with_offset(app.settings_modal.plugins_scroll);

    frame.render_stateful_widget(
        List::new(items)
            .block(focused_block(
                " Plugins — Space to toggle  Enter to Apply ",
                is_focused,
                theme,
            ))
            .highlight_style(Style::default().bg(theme.surface)),
        area,
        &mut state,
    );
    app.settings_modal.plugins_scroll = state.offset();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn focused_block<'a>(title: &'a str, focused: bool, theme: &Theme) -> Block<'a> {
    let border_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.border)
    };
    let title_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let mut block = Block::default()
        .title(Line::styled(title, title_style))
        .borders(Borders::ALL)
        .border_style(border_style);
    if focused {
        block = block
            .border_type(BorderType::Thick)
            .border_set(border::THICK);
    }
    block
}

/// Use the same subdued/active treatment for every settings value.
fn setting_value_style(focused: bool, theme: &Theme) -> Style {
    Style::default().fg(if focused { theme.text } else { theme.muted })
}

fn validate_path(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let path = std::path::Path::new(text.trim());
    if path.is_absolute() {
        None
    } else {
        Some("Path must be absolute".to_owned())
    }
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut prev = cursor - 1;
    while prev > 0 && !text.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut next = cursor + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next.min(text.len())
}

fn prev_word_boundary(text: &str, cursor: usize) -> usize {
    let mut pos = cursor.min(text.len());
    while pos > 0 && text[prev_char_boundary(text, pos)..pos].chars().next().is_some_and(char::is_whitespace) {
        pos = prev_char_boundary(text, pos);
    }
    let word = pos > 0 && text[prev_char_boundary(text, pos)..pos].chars().next().is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
    while pos > 0 {
        let previous = prev_char_boundary(text, pos);
        let ch = text[previous..pos].chars().next().unwrap_or(' ');
        if ch.is_whitespace() || (ch.is_alphanumeric() || ch == '_') != word { break; }
        pos = previous;
    }
    pos
}

fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut pos = cursor.min(text.len());
    if pos >= text.len() { return text.len(); }
    let first = text[pos..].chars().next().unwrap_or(' ');
    if first.is_whitespace() {
        while pos < text.len() && text[pos..].chars().next().is_some_and(char::is_whitespace) {
            pos = next_char_boundary(text, pos);
        }
    } else {
        let word = first.is_alphanumeric() || first == '_';
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or(' ');
            if ch.is_whitespace() || (ch.is_alphanumeric() || ch == '_') != word { break; }
            pos = next;
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_tab_wrapping() {
        assert_eq!(SettingsTab::Theme.next(), SettingsTab::General);
        assert_eq!(SettingsTab::General.next(), SettingsTab::Paths);
        assert_eq!(SettingsTab::Paths.next(), SettingsTab::Plugins);
        assert_eq!(SettingsTab::Plugins.next(), SettingsTab::Theme);

        assert_eq!(SettingsTab::Theme.prev(), SettingsTab::Plugins);
        assert_eq!(SettingsTab::Plugins.prev(), SettingsTab::Paths);
    }

    #[test]
    fn test_staged_config_building() {
        let mut modal = SettingsModalState::default();
        modal.startup_page_selected = 1;
        modal.download_path = "/tmp/downloads".into();
        modal.projects_directory = "/tmp/projects".into();
        modal.library_entries = vec![
            PathEntryState::new("/home/user/papers".into()),
            PathEntryState::new("".into()),
        ];
        modal.enabled_plugins = vec!["plugin-a".into()];

        let options = vec!["dashboard".into(), "discover".into(), "library".into()];
        let base = Config::default();
        let config = staged_config(&modal, &options, &base);

        assert_eq!(config.startup_page, "discover");
        assert_eq!(config.download_path, Some(PathBuf::from("/tmp/downloads")));
        assert_eq!(
            config.projects_directory,
            Some(PathBuf::from("/tmp/projects"))
        );
        assert_eq!(
            config.library_folders,
            vec![PathBuf::from("/home/user/papers")]
        );
        assert_eq!(config.enabled_plugins, vec!["plugin-a"]);
    }

    #[test]
    fn test_path_validation() {
        assert_eq!(validate_path(""), None);
        assert_eq!(validate_path("/absolute/path"), None);
        assert_eq!(
            validate_path("relative/path"),
            Some("Path must be absolute".to_owned())
        );
    }

    #[test]
    fn settings_text_fields_keep_layout_resolved_shifted_characters() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;

        for character in ['A', '!', '?', '_'] {
            let result = handle_settings_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::SHIFT),
            );
            assert!(matches!(result, SettingsKeyResult::Handled));
            assert!(app.settings_modal.pdf_viewer.ends_with(character));
        }
        assert_eq!(app.settings_modal.pdf_viewer, "A!?_");
    }

    #[test]
    fn test_theme_preview_reverts_on_tab_switch() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Theme;
        app.settings_modal.tab_bar_focused = true;
        app.settings_modal.original_theme = "catppuccin-mocha".into();

        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let res = handle_settings_key(&mut app, key_right);

        assert_eq!(app.settings_modal.tab, SettingsTab::General);
        if let SettingsKeyResult::PreviewTheme(reverted) = res {
            assert_eq!(reverted, "catppuccin-mocha");
        } else {
            panic!("Expected PreviewTheme with original theme on tab switch from Theme");
        }
    }

    #[test]
    fn theme_tab_left_arrow_returns_to_sidebar() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Theme;
        app.settings_modal.tab_bar_focused = true;

        let result = handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );

        assert!(matches!(result, SettingsKeyResult::ReturnToSidebar));
        assert_eq!(app.settings_modal.tab, SettingsTab::Theme);
        assert!(app.settings_modal.tab_bar_focused);
    }

    #[test]
    fn theme_list_left_arrow_returns_to_sidebar() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Theme;
        app.settings_modal.tab_bar_focused = false;

        let result = handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );

        assert!(matches!(result, SettingsKeyResult::ReturnToSidebar));
        assert_eq!(app.settings_modal.tab, SettingsTab::Theme);
        assert!(!app.settings_modal.tab_bar_focused);
    }

    #[test]
    fn test_theme_list_left_right_cycles_tabs() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Theme;
        app.settings_modal.tab_bar_focused = false; // Focused inside Theme list
        app.settings_modal.original_theme = "catppuccin-mocha".into();

        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let res = handle_settings_key(&mut app, key_right);

        assert_eq!(app.settings_modal.tab, SettingsTab::General);
        assert!(app.settings_modal.tab_bar_focused);
        if let SettingsKeyResult::PreviewTheme(reverted) = res {
            assert_eq!(reverted, "catppuccin-mocha");
        } else {
            panic!("Expected PreviewTheme with original theme on Right key in Theme list");
        }
    }

    #[test]
    fn test_quit_key_behavior() {
        let mut app = App::default();
        let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        // Outside edit mode -> returns SettingsKeyResult::Quit
        let res = handle_settings_key(&mut app, key_q);
        assert!(matches!(res, SettingsKeyResult::Quit));

        // Inside text edit mode -> types 'q' into text input
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;
        app.settings_modal.pdf_viewer = String::new();
        app.settings_modal.pdf_viewer_cursor = 0;

        let res_edit = handle_settings_key(&mut app, key_q);
        assert!(matches!(res_edit, SettingsKeyResult::Handled));
        assert_eq!(app.settings_modal.pdf_viewer, "q");
        assert!(app.settings_modal.pdf_viewer_editing);
    }

    #[test]
    fn text_fields_delete_words_with_control_backspace_and_delete() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;
        app.settings_modal.pdf_viewer = "alpha, beta".into();
        app.settings_modal.pdf_viewer_cursor = app.settings_modal.pdf_viewer.len();

        let _ = handle_settings_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(app.settings_modal.pdf_viewer, "alpha, ");

        app.settings_modal.pdf_viewer_cursor = 0;
        let _ = handle_settings_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        assert_eq!(app.settings_modal.pdf_viewer, ", ");
    }

    #[test]
    fn slash_opens_discovery_unless_a_settings_text_field_is_being_edited() {
        let mut app = App::default();
        app.page = papr_core::Page::Settings;
        app.content_focused = true;

        let result = handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );

        assert!(matches!(result, SettingsKeyResult::Handled));
        assert_eq!(app.page, papr_core::Page::Discover);
        assert_eq!(app.mode, AppMode::Search);
        assert!(app.content_focused);

        app.page = papr_core::Page::Settings;
        app.mode = AppMode::Normal;
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;
        app.settings_modal.pdf_viewer.clear();
        app.settings_modal.pdf_viewer_cursor = 0;

        let result = handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );

        assert!(matches!(result, SettingsKeyResult::Handled));
        assert_eq!(app.page, papr_core::Page::Settings);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.settings_modal.pdf_viewer, "/");
    }

    #[test]
    fn test_startup_page_consumes_left_right_without_tab_switch() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::StartupPage;
        app.settings_modal.startup_page_selected = 0;

        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let res = handle_settings_key(&mut app, key_right);

        // Tab should remain General, while startup_page_selected advances to 1 without saving immediately
        assert_eq!(app.settings_modal.tab, SettingsTab::General);
        assert_eq!(app.settings_modal.startup_page_selected, 1);
        assert!(matches!(res, SettingsKeyResult::Handled));

        // Pressing Enter applies the selected startup page change
        let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let res_enter = handle_settings_key(&mut app, key_enter);
        assert!(matches!(res_enter, SettingsKeyResult::Apply));
    }

    #[test]
    fn test_universal_left_right_tab_cycling() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Plugins;
        app.settings_modal.tab_bar_focused = false; // Focused in Plugins list

        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let res = handle_settings_key(&mut app, key_right);

        // Right from Plugins wraps around to Theme
        assert_eq!(app.settings_modal.tab, SettingsTab::Theme);
        assert!(app.settings_modal.tab_bar_focused);
        assert!(matches!(res, SettingsKeyResult::Handled));
    }

    #[test]
    fn test_immediate_path_edit_apply_and_validation_failure() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Paths;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.paths_focus = PathsTabFocus::DownloadPath;
        app.settings_modal.download_path_editing = true;

        // 1. Valid absolute path edit -> returns SettingsKeyResult::Apply immediately
        app.settings_modal.download_path = "/valid/download/dir".into();
        let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let res_valid = handle_settings_key(&mut app, key_enter);
        assert!(matches!(res_valid, SettingsKeyResult::Apply));
        assert_eq!(app.settings_modal.download_path_error, None);
        assert!(!app.settings_modal.download_path_editing);

        // 2. Invalid relative path edit -> returns SettingsKeyResult::Handled, sets inline error
        app.settings_modal.download_path_editing = true;
        app.settings_modal.download_path = "relative/download/dir".into();
        let res_invalid = handle_settings_key(&mut app, key_enter);
        assert!(matches!(res_invalid, SettingsKeyResult::Handled));
        assert_eq!(
            app.settings_modal.download_path_error,
            Some("Path must be absolute".to_string())
        );
    }

    #[test]
    fn test_immediate_library_folder_actions_apply() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::Paths;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.paths_focus = PathsTabFocus::LibraryFolders;
        app.settings_modal.library_entries = vec![
            PathEntryState::new("/home/user/papers1".into()),
            PathEntryState::new("/home/user/papers2".into()),
        ];
        app.settings_modal.library_selected = 0;

        // Delete action 'd' -> returns SettingsKeyResult::Apply immediately
        let key_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let res_d = handle_settings_key(&mut app, key_d);
        assert!(matches!(res_d, SettingsKeyResult::Apply));
        assert_eq!(app.settings_modal.library_entries.len(), 1);

        // Reorder action 'J' -> returns SettingsKeyResult::Apply immediately
        app.settings_modal
            .library_entries
            .push(PathEntryState::new("/home/user/papers3".into()));
        app.settings_modal.library_selected = 0;
        let key_j_upper = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE);
        let res_j = handle_settings_key(&mut app, key_j_upper);
        assert!(matches!(res_j, SettingsKeyResult::Apply));
        assert_eq!(
            app.settings_modal.library_entries[0].text,
            "/home/user/papers3"
        );
    }

    #[test]
    fn test_theme_selection_always_syncs_with_applied_theme() {
        let mut app = App::default();
        app.settings_modal.original_theme = "dracula".into();

        // 1. Entering Theme tab should move selection to "dracula"
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = true;
        let key_left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let _ = handle_settings_key(&mut app, key_left);

        assert_eq!(app.settings_modal.tab, SettingsTab::Theme);
        let dracula_idx = Theme::BUILTIN_THEMES
            .iter()
            .position(|&t| t.eq_ignore_ascii_case("dracula"))
            .unwrap();
        assert_eq!(app.settings_modal.theme_selected, dracula_idx);

        // 2. Previewing another theme (moving down 2 items)
        app.settings_modal.tab_bar_focused = false;
        let key_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let _ = handle_settings_key(&mut app, key_down);
        assert_eq!(app.settings_modal.theme_selected, dracula_idx + 1);

        // 3. Leaving Theme tab without applying should revert selection back to "dracula"
        let key_right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let _ = handle_settings_key(&mut app, key_right);
        assert_eq!(app.settings_modal.theme_selected, dracula_idx);
    }

    #[test]
    fn test_dashboard_keywords_actions_and_validation() {
        let mut app = App::default();
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::DashboardKeywords;
        app.settings_modal.keyword_entries = vec![
            PathEntryState::new("quantum computing".into()),
            PathEntryState::new("machine learning".into()),
        ];
        app.settings_modal.keyword_selected = 0;

        // 1. Reorder 'J' -> swaps entries and returns Apply immediately
        let key_j = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE);
        let res_j = handle_settings_key(&mut app, key_j);
        assert!(matches!(res_j, SettingsKeyResult::Apply));
        assert_eq!(
            app.settings_modal.keyword_entries[0].text,
            "machine learning"
        );

        // 2. Add 'a' -> appends empty entry and enters editing
        let key_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let _ = handle_settings_key(&mut app, key_a);
        assert_eq!(app.settings_modal.keyword_entries.len(), 3);
        assert!(app.settings_modal.keyword_editing);
        assert_eq!(app.settings_modal.keyword_selected, 2);

        // 3. Submitting empty string -> returns Handled, shows error, stays editing
        let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let res_empty = handle_settings_key(&mut app, key_enter);
        assert!(matches!(res_empty, SettingsKeyResult::Handled));
        assert!(app.settings_modal.keyword_editing);
        assert_eq!(
            app.settings_modal.keyword_entries[2].error,
            Some("Keyword cannot be empty".to_string())
        );

        // 4. Typing a valid keyword and hitting Enter -> returns Apply and exits editing
        let key_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let _ = handle_settings_key(&mut app, key_g);
        let res_valid = handle_settings_key(&mut app, key_enter);
        assert!(matches!(res_valid, SettingsKeyResult::Apply));
        assert!(!app.settings_modal.keyword_editing);
        assert_eq!(app.settings_modal.keyword_entries[2].text, "g");

        // 5. Delete 'd' -> removes entry and returns Apply
        let key_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let res_d = handle_settings_key(&mut app, key_d);
        assert!(matches!(res_d, SettingsKeyResult::Apply));
        assert_eq!(app.settings_modal.keyword_entries.len(), 2);
    }

    #[test]
    fn test_ctrl_b_shortcut_handling() {
        let mut app = App::default();
        let key_ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);

        // Outside text edit mode -> dispatches Command::TogglePalette and returns Handled
        let res = handle_settings_key(&mut app, key_ctrl_b);
        assert!(matches!(res, SettingsKeyResult::Handled));
        assert_eq!(app.mode, AppMode::CommandPalette);

        // Reset mode
        app.mode = AppMode::Normal;

        // Inside text edit mode -> does not interrupt text edit mode
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;

        let _ = handle_settings_key(&mut app, key_ctrl_b);
        assert_ne!(app.mode, AppMode::CommandPalette);
        assert!(app.settings_modal.pdf_viewer_editing);
    }

    #[test]
    fn test_question_mark_shortcut_handling() {
        let mut app = App::default();
        let key_question = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::SHIFT);

        // Outside text edit mode -> dispatches Command::ToggleHelp and returns Handled
        let res = handle_settings_key(&mut app, key_question);
        assert!(matches!(res, SettingsKeyResult::Handled));
        assert_eq!(app.mode, AppMode::Help);

        // Reset mode
        app.mode = AppMode::Normal;

        // Inside text edit mode -> types '?' into text box instead of toggling Help
        app.settings_modal.tab = SettingsTab::General;
        app.settings_modal.tab_bar_focused = false;
        app.settings_modal.general_focus = GeneralTabFocus::PdfViewer;
        app.settings_modal.pdf_viewer_editing = true;
        app.settings_modal.pdf_viewer = "zathura".to_string();
        app.settings_modal.pdf_viewer_cursor = 7;

        let _ = handle_settings_key(&mut app, key_question);
        assert_ne!(app.mode, AppMode::Help);
        assert!(app.settings_modal.pdf_viewer_editing);
        assert_eq!(app.settings_modal.pdf_viewer, "zathura?");
    }
}
