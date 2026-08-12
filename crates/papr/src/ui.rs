//! Ratatui rendering for the application shell.

use crate::state::{
    App, AppMode, CollectionSearchItem, DeletionTarget, DiscoveryStatus, DownloadStatus,
    DownloadTask, Page, ProjectCitationSearchMode, ProjectPane,
};
use crate::theme::Theme;
use std::{fmt::Write as _, sync::OnceLock};

use crate::build_config_editor_view_with_scroll_mode;
use crate::settings_modal;
use num_traits::ToPrimitive;
use papr_core::{ProjectDiagnosticSeverity, RemotePaper};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Widget, Wrap,
    },
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color as SyntectColor, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

const LOGO: &str = "[ P A P R ]";
const WORKSPACE_LIST_ITEM_GAP_ROWS: usize = 1;
const CITATION_BADGE: &str = " (Added)";

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// Build a workspace list item with the standard vertical separation.
///
/// Keeping the gap here means item renderers only describe their content.
fn workspace_list_item(mut lines: Vec<Line<'_>>) -> ListItem<'_> {
    lines.extend(std::iter::repeat_n(
        Line::raw(""),
        WORKSPACE_LIST_ITEM_GAP_ROWS,
    ));
    ListItem::new(lines)
}

fn markdown_syntax_assets() -> &'static (SyntaxSet, ThemeSet) {
    static ASSETS: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    ASSETS.get_or_init(|| {
        (
            SyntaxSet::load_defaults_newlines(),
            ThemeSet::load_defaults(),
        )
    })
}

fn focus_block<'a>(title: &'a str, focused: bool, theme: &Theme) -> Block<'a> {
    let border_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border)
    };

    let title_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
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

/// A pane header with independent left and right titles, avoiding text overlap.
fn focus_block_with_right_title<'a>(
    left_title: &'a str,
    right_title: &'a str,
    focused: bool,
    theme: &Theme,
) -> Block<'a> {
    let right_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    focus_block(left_title, focused, theme)
        .title_top(Line::styled(right_title, right_style).alignment(Alignment::Right))
}

/// Return a list selection only while the Workspace owns keyboard focus.
///
/// The selected index remains in the application state at all times; omitting it
/// here only prevents Ratatui from drawing the selection treatment.
fn workspace_highlight_selection(app: &App, selected: usize) -> Option<usize> {
    app.content_focused.then_some(selected)
}

/// Resolve the user-visible timezone label for a local timestamp.
///
/// Priority:
/// 1. System timezone abbreviation if available from OS (e.g. IST, CEST, EDT, JST).
/// 2. Comprehensive IANA timezone lookup table mapping to standard abbreviations.
/// 3. Fallback to UTC offset enclosed in parentheses, e.g. `(+05:30)` or `(-07:00)`.
#[must_use]
pub fn resolve_timezone_display(local: &chrono::DateTime<chrono::Local>) -> String {
    let raw_tz = local.format("%Z").to_string();
    let trimmed = raw_tz.trim();

    // Check if OS provided a valid alphabetic abbreviation (e.g., IST, CEST, EDT, PDT, JST, UTC, AEST, BST, CET).
    if !trimmed.is_empty()
        && trimmed.len() <= 6
        && trimmed.chars().all(|c| c.is_ascii_alphabetic())
        && !trimmed.eq_ignore_ascii_case("LOCAL")
    {
        return trimmed.to_uppercase();
    }

    // Try mapping detected system IANA timezone identifier (e.g. "Asia/Kolkata", "America/New_York") to standard abbreviation.
    if let Ok(iana_tz) = iana_time_zone::get_timezone()
        && let Some(abbr) = iana_to_abbreviation(&iana_tz, local)
    {
        return abbr.to_string();
    }

    // Fallback: UTC offset enclosed in parentheses (e.g., (+05:30) or (-07:00))
    format!("({})", local.format("%:z"))
}

fn iana_to_abbreviation(
    iana: &str,
    local: &chrono::DateTime<chrono::Local>,
) -> Option<&'static str> {
    use chrono::Offset;
    let offset_secs = local.offset().fix().local_minus_utc();

    asia_pacific_abbreviation(iana, offset_secs)
        .or_else(|| american_abbreviation(iana, offset_secs))
        .or_else(|| european_abbreviation(iana, offset_secs))
}

fn asia_pacific_abbreviation(iana: &str, offset_secs: i32) -> Option<&'static str> {
    match iana {
        "Asia/Kolkata" | "Asia/Calcutta" | "Asia/Colombo" => Some("IST"),
        "Asia/Tokyo" => Some("JST"),
        "Asia/Seoul" => Some("KST"),
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "Asia/Urumqi" | "Asia/Taipei" => {
            Some("CST")
        }
        "Asia/Hong_Kong" => Some("HKT"),
        "Asia/Singapore" => Some("SGT"),
        "Asia/Manila" => Some("PST"),
        "Asia/Kuala_Lumpur" | "Asia/Kuching" => Some("MYT"),
        "Australia/Sydney" | "Australia/Melbourne" | "Australia/Canberra" | "Australia/Hobart" => {
            if offset_secs == 11 * 3600 {
                Some("AEDT")
            } else {
                Some("AEST")
            }
        }
        "Australia/Brisbane" => Some("AEST"),
        "Australia/Adelaide" => {
            if offset_secs == 10 * 3600 + 1800 {
                Some("ACDT")
            } else {
                Some("ACST")
            }
        }
        "Australia/Darwin" => Some("ACST"),
        "Australia/Perth" => Some("AWST"),
        "Pacific/Auckland" | "NZ" => {
            if offset_secs == 13 * 3600 {
                Some("NZDT")
            } else {
                Some("NZST")
            }
        }
        "Asia/Dubai" => Some("GST"),
        "Asia/Riyadh" => Some("AST"),
        _ => None,
    }
}

fn american_abbreviation(iana: &str, offset_secs: i32) -> Option<&'static str> {
    match iana {
        "America/New_York"
        | "America/Detroit"
        | "America/Toronto"
        | "America/Montreal"
        | "America/Indiana/Indianapolis"
        | "US/Eastern" => {
            if offset_secs == -4 * 3600 {
                Some("EDT")
            } else {
                Some("EST")
            }
        }
        "America/Chicago" | "America/Winnipeg" | "America/Mexico_City" | "US/Central" => {
            if offset_secs == -5 * 3600 {
                Some("CDT")
            } else {
                Some("CST")
            }
        }
        "America/Denver" | "America/Edmonton" | "America/Boise" | "US/Mountain" => {
            if offset_secs == -6 * 3600 {
                Some("MDT")
            } else {
                Some("MST")
            }
        }
        "America/Phoenix" => Some("MST"),
        "America/Los_Angeles" | "America/Vancouver" | "America/Tijuana" | "US/Pacific" => {
            if offset_secs == -7 * 3600 {
                Some("PDT")
            } else {
                Some("PST")
            }
        }
        "America/Anchorage" => {
            if offset_secs == -8 * 3600 {
                Some("AKDT")
            } else {
                Some("AKST")
            }
        }
        "Pacific/Honolulu" => Some("HST"),
        _ => None,
    }
}

fn european_abbreviation(iana: &str, offset_secs: i32) -> Option<&'static str> {
    match iana {
        "Europe/London" | "Europe/Belfast" | "Europe/Dublin" => {
            if offset_secs == 3600 {
                Some("BST")
            } else {
                Some("GMT")
            }
        }
        "Europe/Berlin" | "Europe/Paris" | "Europe/Rome" | "Europe/Madrid" | "Europe/Amsterdam"
        | "Europe/Brussels" | "Europe/Vienna" | "Europe/Zurich" | "Europe/Stockholm"
        | "Europe/Oslo" | "Europe/Copenhagen" | "Europe/Prague" | "Europe/Warsaw"
        | "Europe/Budapest" | "Europe/Belgrade" => {
            if offset_secs == 2 * 3600 {
                Some("CEST")
            } else {
                Some("CET")
            }
        }
        "Europe/Athens" | "Europe/Helsinki" | "Europe/Bucharest" | "Europe/Kiev"
        | "Europe/Sofia" | "Europe/Tallinn" | "Europe/Riga" | "Europe/Vilnius" => {
            if offset_secs == 3 * 3600 {
                Some("EEST")
            } else {
                Some("EET")
            }
        }
        "Europe/Moscow" => Some("MSK"),
        "Europe/Istanbul" => Some("TRT"),
        "UTC" | "Etc/UTC" | "Etc/GMT" | "GMT" => Some("UTC"),
        _ => None,
    }
}

/// Format a UTC `DateTime` in the system's local timezone with timezone abbreviation or parenthesized offset.
pub fn format_local_datetime(dt: &chrono::DateTime<chrono::Utc>, fmt: &str) -> String {
    let local = dt.with_timezone(&chrono::Local);
    let tz = resolve_timezone_display(&local);
    format!("{} {}", local.format(fmt), tz)
}

/// Render the complete application for the current state.
pub fn render(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    if app.mode == AppMode::PdfView {
        crate::pdf_viewer::draw_pdf_viewer(frame, app);
        return;
    }
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );
    if area.width < 58 || area.height < 18 {
        render_too_small(frame, area, theme);
        crate::pdf_viewer::render_pending_kitty_cleanup(frame);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(frame, rows[0], app, theme);
    let project_open = app.page == Page::Projects
        && app.active_project.is_some()
        && app.project_pane != ProjectPane::ProjectList;
    if project_open {
        let pdf_occlusion = (app.mode == AppMode::ProjectCitationSearch)
            .then(|| project_citation_search_area(area));
        render_projects(frame, rows[1], app, theme, pdf_occlusion);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(32)])
            .split(rows[1]);
        render_sidebar(frame, columns[0], app, theme);
        render_content(frame, columns[1], app, theme);
    }
    render_status(frame, rows[2], app, theme);

    match app.mode {
        AppMode::CommandPalette => render_palette(frame, app, theme),
        AppMode::TerminalCommand => render_terminal_command(frame, app, theme),
        AppMode::Help => render_help(frame, app, theme),
        AppMode::PaperDetail => render_paper_detail(frame, app, theme),
        AppMode::NoteEdit => render_note_editor(frame, app, theme),
        AppMode::Prompt => render_metadata_prompt(frame, app, theme),
        AppMode::ConfirmDelete => render_delete_confirmation(frame, app, theme),
        AppMode::ProjectRename
        | AppMode::ProjectCreate
        | AppMode::ProjectFileCreate
        | AppMode::ProjectEntryRename => render_project_name_prompt(frame, app, theme),
        AppMode::ProjectCitationSearch => render_project_citation_search(frame, app, theme),
        AppMode::SummaryModal => render_summary_modal(frame, app, theme),
        AppMode::Normal
        | AppMode::Search
        | AppMode::DiscoverFilter
        | AppMode::WorkspaceSearch
        | AppMode::PdfView
        | AppMode::SettingsModal => {}
    }

    let project_editor_focused = app.page == Page::Projects
        && app.mode == AppMode::Normal
        && app.content_focused
        && app.project_pane == ProjectPane::Editor;
    let config_editor_focused = app.page == Page::Settings
        && app.mode == AppMode::Normal
        && app.overlay_flags.config_editor_focused;
    let cursor_style = if config_editor_focused || project_editor_focused {
        if project_editor_focused && app.project_editor_pending_sequence.is_some() {
            crossterm::cursor::SetCursorStyle::SteadyUnderScore
        } else if app.overlay_flags.config_editor_insert_mode
            || (project_editor_focused && app.project_editor_insert_mode)
        {
            crossterm::cursor::SetCursorStyle::BlinkingBar
        } else {
            crossterm::cursor::SetCursorStyle::BlinkingBlock
        }
    } else if app.mode == AppMode::SettingsModal {
        // The settings modal manages cursor visibility itself.
        crossterm::cursor::SetCursorStyle::BlinkingBar
    } else {
        crossterm::cursor::SetCursorStyle::BlinkingBar
    };
    crate::pdf_viewer::render_pending_kitty_cleanup(frame);
    let _ = crossterm::execute!(std::io::stdout(), cursor_style);
}

fn render_project_name_prompt(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let is_create = app.mode == AppMode::ProjectCreate;
    let height = if is_create { 8 } else { 4 };
    let area = frame.area();
    let popup = Rect::new(
        area.x + area.width / 4,
        area.y
            .saturating_add(area.height / 2)
            .saturating_sub(height / 2),
        area.width / 2,
        height,
    );
    let title = match app.mode {
        AppMode::ProjectCreate => " NEW PROJECT — ENTER CREATE  ESC CANCEL ",
        AppMode::ProjectFileCreate => " NEW FILE OR FOLDER — ENTER CREATE  ESC CANCEL ",
        AppMode::ProjectEntryRename => " RENAME FILE OR FOLDER — ENTER SAVE  ESC CANCEL ",
        AppMode::ProjectRename => " RENAME PROJECT — ENTER SAVE  ESC CANCEL ",
        _ => unreachable!("project prompt rendered outside a project prompt mode"),
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface)),
        popup,
    );

    if is_create {
        let chunks = Layout::vertical([Constraint::Length(3), Constraint::Length(3)])
            .margin(1)
            .split(popup);

        frame.render_widget(
            Paragraph::new(app.project_rename_input.as_str())
                .block(focus_block(title, true, theme)),
            chunks[0],
        );

        let compiler_label = match app.project_create_compiler.as_str() {
            "typst" => "Typst",
            _ => "LaTeX",
        };
        let compiler_text = format!(" ◄  {compiler_label}  ► ");
        frame.render_widget(
            Paragraph::new(Line::styled(compiler_text, Style::default().fg(theme.text)))
                .block(focus_block(" Compiler (Tab to cycle) ", false, theme)),
            chunks[1],
        );

        let cursor_columns = app.project_rename_input[..app
            .project_rename_cursor
            .min(app.project_rename_input.len())]
            .chars()
            .count();
        let cursor_columns = saturating_u16(cursor_columns);
        frame.set_cursor_position((
            chunks[0].x.saturating_add(1 + cursor_columns),
            chunks[0].y + 1,
        ));
    } else {
        frame.render_widget(
            Paragraph::new(app.project_rename_input.as_str())
                .style(Style::default().bg(theme.surface))
                .block(focus_block(title, true, theme)),
            popup,
        );
        let cursor_columns = app.project_rename_input[..app
            .project_rename_cursor
            .min(app.project_rename_input.len())]
            .chars()
            .count();
        let cursor_columns = saturating_u16(cursor_columns);
        frame.set_cursor_position((popup.x.saturating_add(1 + cursor_columns), popup.y + 1));
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let title = Line::from(vec![
        Span::styled(
            LOGO,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            app.page.title(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    frame.render_widget(Paragraph::new(title).block(block), area);
    let shortcut = Rect::new(area.x + area.width.saturating_sub(18), area.y, 18, 1);
    frame.render_widget(
        Paragraph::new("Ctrl+B  Browse")
            .alignment(Alignment::Right)
            .style(Style::default().fg(theme.muted)),
        shortcut,
    );
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let focused = !app.content_focused;
    let block = focus_block(" NAVIGATION ", focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = Page::ALL
        .iter()
        .map(|page| ListItem::new(format!("  {}", page.title())));
    let list = List::new(items)
        .style(Style::default().fg(theme.muted))
        .highlight_style(if app.content_focused {
            Style::default().fg(theme.text).bg(theme.surface)
        } else {
            Style::default()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        })
        .highlight_symbol("›");
    let mut state = ListState::default()
        .with_selected(Some(app.sidebar_index))
        .with_offset(app.sidebar_scroll);
    frame.render_stateful_widget(list, inner, &mut state);
    app.sidebar_scroll = state.offset();
}

fn render_content(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let block = focus_block(" WORKSPACE ", app.content_focused, theme);
    let outer = block.inner(area);
    frame.render_widget(block, area);

    let inset = Rect::new(
        outer.x + 1,
        outer.y,
        outer.width.saturating_sub(2),
        outer.height,
    );
    let mut workspace_area = inset;
    if app.page.supports_workspace_search() {
        let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(inset);
        render_workspace_search_bar(frame, rows[0], app, theme);
        workspace_area = rows[1];
    }

    if app.page == Page::Dashboard {
        render_dashboard(frame, inset, app, theme);
    } else if app.page == Page::Projects {
        render_projects(frame, inset, app, theme, None);
    } else if app.page == Page::Discover {
        render_discover(frame, inset, app, theme);
    } else if app.page == Page::Library {
        render_library(frame, workspace_area, app, theme);
    } else if app.page == Page::ReadingQueue {
        render_reading_queue(frame, workspace_area, app, theme);
    } else if app.page == Page::Downloads {
        render_downloads(frame, workspace_area, app, theme);
    } else if app.page == Page::Collections {
        render_collections(frame, workspace_area, app, theme);
    } else if app.page == Page::Authors {
        render_authors(frame, workspace_area, app, theme);
    } else if app.page == Page::Bookmarks || app.page == Page::Notes {
        render_organization(frame, workspace_area, app, theme);
    } else if app.page == Page::History {
        render_history(frame, inset, app, theme);
    } else if app.page == Page::Statistics {
        render_statistics(frame, inset, app, theme);
    } else if app.page == Page::Settings {
        render_settings(frame, inset, app, theme);
    } else if app.page == Page::Credits {
        render_credits(frame, inset, app, theme);
    } else {
        let lines = vec![
            Line::styled(
                app.page.title(),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                "This workspace is ready for its Milestone module.",
                Style::default().fg(theme.muted),
            ),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inset);
    }
}

fn render_projects(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    pdf_occlusion: Option<Rect>,
) {
    if app.active_project.is_none() || app.project_pane == ProjectPane::ProjectList {
        render_project_list(frame, area, app, theme);
        return;
    }
    let Some(project_path) = app
        .active_project
        .as_ref()
        .map(|project| project.path.clone())
    else {
        return;
    };
    let (file_tree_area, editor_area, right_area) = project_workspace_areas(area, app);
    render_project_file_tree(frame, file_tree_area, app, theme, &project_path);
    render_project_editor(frame, editor_area, app, theme);
    render_project_right_pane(frame, right_area, app, theme, &project_path, pdf_occlusion);
}

fn render_project_list(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items = app.projects.iter().map(|project| {
        workspace_list_item(vec![
            Line::styled(
                &project.name,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                project.path.display().to_string(),
                Style::default().fg(theme.muted),
            ),
        ])
    });
    let list = List::new(items)
        .block(focus_block(
            " PROJECTS — n NEW  ENTER/RIGHT OPEN  x DELETE  r REFRESH ",
            app.content_focused,
            theme,
        ))
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.projects_selected));
    frame.render_stateful_widget(list, area, &mut state);
    if app.projects.is_empty() {
        frame.render_widget(
            Paragraph::new("No projects yet. Press n to create one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
    }
}

fn project_workspace_areas(area: Rect, app: &App) -> (Rect, Rect, Option<Rect>) {
    if app.pdf_viewer == "internal" || app.project_view_flags.build_visible {
        let panes = Layout::horizontal([
            Constraint::Length(22),
            Constraint::Ratio(1, 2),
            Constraint::Ratio(1, 2),
        ])
        .split(area);
        (panes[0], panes[1], Some(panes[2]))
    } else {
        let panes = Layout::horizontal([Constraint::Length(22), Constraint::Min(10)]).split(area);
        (panes[0], panes[1], None)
    }
}

fn render_project_file_tree(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    theme: &Theme,
    project_path: &std::path::Path,
) {
    let files = app.project_files.iter().map(|path| {
        let mut label = path
            .strip_prefix(project_path)
            .unwrap_or(path)
            .display()
            .to_string();
        if path.is_dir() {
            label.push('/');
        }
        ListItem::new(label)
    });
    let file_tree_focused = app.content_focused && app.project_pane == ProjectPane::FileTree;
    let tree_block = focus_block(" FILE TREE [ALT+1] ", file_tree_focused, theme).title_bottom(
        Line::styled(
            " n new · ↵/→ open ",
            if file_tree_focused {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        )
        .alignment(Alignment::Center),
    );
    let tree = List::new(files)
        .block(tree_block)
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.project_file_selected));
    frame.render_stateful_widget(tree, area, &mut state);
}

fn render_project_editor(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let editor_title = app
        .project_editor_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Editor");
    let editor_height = area.height.saturating_sub(2) as usize;
    app.project_editor_wrap_width = area.width.saturating_sub(6).max(1) as usize;
    app.project_editor_viewport_height = editor_height;
    let editor_view = build_config_editor_view_with_scroll_mode(
        &app.project_editor_text,
        app.project_editor_cursor,
        app.project_editor_wrap_width,
        editor_height,
        &mut app.project_editor_scroll,
        !app.project_view_flags.editor_manual_scroll,
    );
    let visual_lines = app.project_editor_visual_line_anchor.map(|anchor| {
        let current = app.project_editor_text
            [..app.project_editor_cursor.min(app.project_editor_text.len())]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        (anchor.min(current), anchor.max(current))
    });
    let mut rendered_source_line = 0_usize;
    for line in editor_view.lines.iter().take(app.project_editor_scroll) {
        if let Ok(line_number) = line[..4.min(line.len())].trim().parse::<usize>() {
            rendered_source_line = line_number.saturating_sub(1);
        }
    }
    let editor_lines = editor_view
        .lines
        .iter()
        .skip(app.project_editor_scroll)
        .take(editor_height)
        .map(|line| {
            let (prefix, content) = line.split_at(4.min(line.len()));
            if let Ok(line_number) = prefix.trim().parse::<usize>() {
                rendered_source_line = line_number.saturating_sub(1);
            }
            let selected = visual_lines
                .is_some_and(|(first, last)| (first..=last).contains(&rendered_source_line));
            let line_style = if selected {
                Style::default().bg(theme.surface).fg(theme.accent)
            } else {
                Style::default().fg(theme.text)
            };
            Line::from(vec![
                Span::styled(
                    prefix.to_owned(),
                    if selected {
                        line_style
                    } else {
                        Style::default().fg(theme.muted)
                    },
                ),
                Span::styled(content.to_owned(), line_style),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(editor_lines)
            .wrap(Wrap { trim: false })
            .block(focus_block(
                &format!(
                    " EDITOR [ALT+2]{} — {editor_title}{} ",
                    if visual_lines.is_some() {
                        " [VISUAL LINE]"
                    } else {
                        ""
                    },
                    if app.project_editor_dirty { " •" } else { "" },
                ),
                app.content_focused && app.project_pane == ProjectPane::Editor,
                theme,
            )),
        area,
    );
    if app.content_focused
        && app.project_pane == ProjectPane::Editor
        && editor_view.cursor_row >= app.project_editor_scroll
        && editor_view.cursor_row < app.project_editor_scroll + editor_height
    {
        frame.set_cursor_position((
            area.x
                .saturating_add(5)
                .saturating_add(u16::try_from(editor_view.cursor_col).unwrap_or(0)),
            area.y.saturating_add(1).saturating_add(
                u16::try_from(
                    editor_view
                        .cursor_row
                        .saturating_sub(app.project_editor_scroll),
                )
                .unwrap_or(0),
            ),
        ));
        render_project_completions(frame, area, app, theme, editor_view.cursor_row);
    }
}

fn render_project_completions(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    theme: &Theme,
    cursor_row: usize,
) {
    if app.project_completions.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(8).clamp(24, 68);
    let height = saturating_u16(app.project_completions.len())
        .saturating_add(2)
        .min(10);
    let popup = Rect::new(
        area.x.saturating_add(4),
        area.y
            .saturating_add(2)
            .saturating_add(
                u16::try_from(cursor_row.saturating_sub(app.project_editor_scroll)).unwrap_or(0),
            )
            .min(area.height.saturating_sub(height)),
        width,
        height,
    );
    let items = app.project_completions.iter().map(|item| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{}  ", item.label),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&item.detail, Style::default().fg(theme.text)),
        ]))
    });
    let list = List::new(items)
        .block(focus_block(" CITATIONS — ENTER/TAB INSERT ", true, theme))
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent));
    let mut state = ListState::default().with_selected(Some(app.project_completion_selected));
    frame.render_widget(Clear, popup);
    frame.render_stateful_widget(list, popup, &mut state);
}

fn render_project_right_pane(
    frame: &mut Frame<'_>,
    area: Option<Rect>,
    app: &mut App,
    theme: &Theme,
    project_path: &std::path::Path,
    pdf_occlusion: Option<Rect>,
) {
    if app.project_view_flags.build_visible {
        let Some(build_area) = area else {
            return;
        };
        render_project_build(frame, build_area, app, theme);
    } else if let Some(preview_area) = area {
        let preview = project_path.join("main.pdf");
        if preview.exists() && app.pdf_viewer_path.as_deref() == Some(preview.as_path()) {
            let preview_block = focus_block_with_right_title(
                " PDF PREVIEW [Alt+3 · Tab BUILD] ",
                " Alt+4 BUILD ",
                app.content_focused && app.project_pane == ProjectPane::Preview,
                theme,
            );
            let p_area = preview_block.inner(preview_area);
            frame.render_widget(preview_block, preview_area);
            crate::pdf_viewer::draw_pdf_viewer_in_with_occlusion(frame, app, p_area, pdf_occlusion);
        } else {
            frame.render_widget(
                Paragraph::new("Live PDF preview\nWaiting for the first successful build…")
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true })
                    .block(focus_block_with_right_title(
                        " PDF PREVIEW [Alt+3 · Tab BUILD] ",
                        " Alt+4 BUILD ",
                        app.content_focused && app.project_pane == ProjectPane::Preview,
                        theme,
                    )),
                preview_area,
            );
        }
    }
}

fn render_project_build(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    app.project_build_viewport_height = area.height.saturating_sub(2) as usize;
    if app.project_view_flags.build_show_raw {
        let raw_log = if app.project_build_raw_log.is_empty() {
            "No compiler output captured yet.".to_owned()
        } else {
            app.project_build_raw_log.join("\n")
        };
        let line_count = app.project_build_raw_log.len().max(1);
        app.project_build_scroll = app
            .project_build_scroll
            .min(line_count.saturating_sub(app.project_build_viewport_height.max(1)));
        frame.render_widget(
            Paragraph::new(raw_log)
                .scroll((
                    u16::try_from(app.project_build_scroll).unwrap_or(u16::MAX),
                    0,
                ))
                .wrap(Wrap { trim: false })
                .block(project_build_block(app, theme)),
            area,
        );
        return;
    }
    if app.project_build_diagnostics.is_empty() {
        frame.render_widget(
            Paragraph::new("No compiler diagnostics.\n\nThe latest build completed cleanly.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted))
                .block(project_build_block(app, theme)),
            area,
        );
        return;
    }
    let items = app
        .project_build_diagnostics
        .iter()
        .map(|diagnostic| project_diagnostic_item(diagnostic, area.width as usize, theme));
    let list = List::new(items)
        .block(project_build_block(app, theme))
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent));
    let mut state = ListState::default().with_selected(Some(app.project_build_selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn project_build_block<'a>(app: &App, theme: &Theme) -> Block<'a> {
    focus_block_with_right_title(
        " BUILD [Alt+4 · Tab PREVIEW] ",
        " Alt+3 PDF PREVIEW ",
        app.content_focused && app.project_pane == ProjectPane::Build,
        theme,
    )
}

fn project_diagnostic_item<'a>(
    diagnostic: &'a papr_core::ProjectBuildDiagnostic,
    width: usize,
    theme: &Theme,
) -> ListItem<'a> {
    let (symbol, label, color) = match diagnostic.severity {
        ProjectDiagnosticSeverity::Error => ("❌", "ERROR", theme.error),
        ProjectDiagnosticSeverity::Warning => ("⚠", "WARNING", theme.warning),
    };
    let mut lines = wrap_text_to_spans(
        &format!("{symbol} {label}: "),
        &diagnostic.title,
        width,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    );
    if !diagnostic.description.is_empty() && diagnostic.description != diagnostic.title {
        lines.extend(wrap_text_to_spans(
            "   ",
            &diagnostic.description,
            width,
            Style::default().fg(theme.muted),
            Style::default().fg(theme.text),
        ));
    }
    if let Some(file) = &diagnostic.file {
        let location = match (diagnostic.line, diagnostic.col) {
            (Some(line), Some(col)) => format!("{file}:{line}:{col}"),
            (Some(line), None) => format!("{file}:{line}"),
            (None, _) => file.clone(),
        };
        lines.extend(wrap_text_to_spans(
            "   File : ",
            &location,
            width,
            Style::default().fg(theme.muted),
            Style::default().fg(theme.muted),
        ));
    }
    for (prefix, value, style) in [
        ("   Code : ", diagnostic.code.as_deref(), theme.accent),
        ("   Hint : ", diagnostic.hint.as_deref(), theme.muted),
    ] {
        if let Some(value) = value {
            lines.extend(wrap_text_to_spans(
                prefix,
                value,
                width,
                Style::default().fg(theme.muted),
                Style::default().fg(style),
            ));
        }
    }
    lines.push(Line::raw(""));
    ListItem::new(lines)
}

fn render_workspace_search_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let style = if app.mode == AppMode::WorkspaceSearch {
        Style::default().fg(theme.text).bg(theme.surface)
    } else {
        Style::default().fg(theme.muted)
    };
    let border_style = if app.mode == AppMode::WorkspaceSearch {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.border)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Local Search (Press: > to toggle) ");

    let mut text = app.workspace_query.clone();
    if text.is_empty() && app.mode != AppMode::WorkspaceSearch {
        "Type to filter...".clone_into(&mut text);
    }
    frame.render_widget(Paragraph::new(text).style(style).block(block), area);

    if app.mode == AppMode::WorkspaceSearch {
        let cursor_offset = app
            .workspace_query
            .chars()
            .take(app.workspace_query_cursor)
            .count();
        frame.set_cursor_position((
            area.x
                .saturating_add(1)
                .saturating_add(u16::try_from(cursor_offset).unwrap_or(0)),
            area.y.saturating_add(1),
        ));
    }
}

fn render_collections(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    if app.active_collection.is_some() {
        render_collection_papers(frame, area, app, theme);
        return;
    }
    let collections = app.filtered_collections();
    if collections.is_empty() {
        frame.render_widget(
            Paragraph::new("No groups yet. Select a paper and press g to create one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let items = collections.iter().map(|item| {
        use CollectionSearchItem;
        match item {
            CollectionSearchItem::Collection(collection) => workspace_list_item(vec![
                Line::styled(
                    &collection.name,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    format!("{} papers", collection.paper_count),
                    Style::default().fg(theme.muted),
                ),
            ]),
            CollectionSearchItem::Paper(paper, _) => workspace_list_item(vec![
                Line::styled(
                    format!("  {}", paper.display_name()),
                    Style::default().fg(theme.text),
                ),
                Line::styled(
                    format!("    {}", paper.authors),
                    Style::default().fg(theme.muted),
                ),
            ]),
        }
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" GROUPS - ENTER VIEW  g NEW  R RENAME x Delete")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.collection_selected))
        .with_offset(app.collection_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.collection_scroll = state.offset();
}

fn render_collection_papers(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let Some(collection) = &app.active_collection else {
        return;
    };
    let papers = app.filtered_collection_papers();
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Groups / ", Style::default().fg(theme.muted)),
            Span::styled(
                &collection.name,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   h/Esc back   Enter/Right/l open PDF",
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[0],
    );
    if papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No papers are assigned to this group.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let available_width = rows[1].width.saturating_sub(2) as usize;
    let items = papers.iter().map(|paper| {
        let availability = if paper.pdf_path.is_some() {
            "PDF available"
        } else {
            "Metadata only"
        };
        let lines = build_paper_lines(
            app,
            theme,
            PaperLineContext {
                paper_id: Some(paper.id),
                title: paper.display_name(),
                authors: &paper.authors,
                reading_status: &paper.reading_status,
                file_size: paper.file_size,
                availability: Some(availability),
                bookmark_year: None,
                bookmark_journal: None,
                bookmark_doi: None,
                bookmark_page: None,
                download_label: None,
                available_width,
            },
        );
        workspace_list_item(lines)
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(
            app,
            app.collection_paper_selected,
        ))
        .with_offset(app.collection_paper_scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.collection_paper_scroll = state.offset();
}

fn render_authors(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    if app.active_author.is_some() {
        render_author_papers(frame, area, app, theme);
        return;
    }
    let authors = app.filtered_authors();
    if authors.is_empty() {
        frame.render_widget(
            Paragraph::new("No authors found in your library.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let items = authors.iter().map(|author| {
        workspace_list_item(vec![
            Line::styled(
                &author.name,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!("{} papers", author.paper_count),
                Style::default().fg(theme.muted),
            ),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} AUTHORS - ENTER VIEW ", authors.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.author_selected))
        .with_offset(app.author_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.author_scroll = state.offset();
}

fn render_author_papers(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let Some(author) = &app.active_author else {
        return;
    };
    let papers = app.filtered_author_papers();
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Authors / ", Style::default().fg(theme.muted)),
            Span::styled(
                &author.name,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   h/Esc back   Enter/Right/l open PDF",
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[0],
    );
    if papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No papers found for this author.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let available_width = rows[1].width.saturating_sub(2) as usize;
    let items = papers.iter().map(|paper| {
        let availability = if paper.pdf_path.is_some() {
            "PDF available"
        } else {
            "Metadata only"
        };
        let lines = build_paper_lines(
            app,
            theme,
            PaperLineContext {
                paper_id: Some(paper.id),
                title: paper.display_name(),
                authors: &paper.authors,
                reading_status: &paper.reading_status,
                file_size: paper.file_size,
                availability: Some(availability),
                bookmark_year: None,
                bookmark_journal: None,
                bookmark_doi: None,
                bookmark_page: None,
                download_label: None,
                available_width,
            },
        );
        workspace_list_item(lines)
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(
            app,
            app.author_paper_selected,
        ))
        .with_offset(app.author_paper_scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.author_paper_scroll = state.offset();
}

fn render_settings(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    settings_modal::render_settings_ui(frame, area, app, theme);
}

fn render_history(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let items = app.dashboard.recent_activity.iter().map(|activity| {
        workspace_list_item(vec![
            Line::styled(
                &activity.label,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!(
                    "{}  {}",
                    activity_kind(&activity.kind),
                    format_local_datetime(&activity.occurred_at, "%Y-%m-%d %H:%M")
                ),
                Style::default().fg(theme.muted),
            ),
        ])
    });
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" READING AND RESEARCH ACTIVITY ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        ),
        area,
    );
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds.to_f64().unwrap_or(f64::MAX) / 60.0;
    if minutes < 60.0 {
        format_decimal(minutes, "min")
    } else {
        let hours = minutes / 60.0;
        if hours < 24.0 {
            format_decimal(hours, "h")
        } else {
            let days = hours / 24.0;
            if days < 30.0 {
                format_decimal(days, "days")
            } else {
                let months = days / 30.0;
                if months < 12.0 {
                    format_decimal(months, "months")
                } else {
                    let years = days / 365.0;
                    format_decimal(years, "years")
                }
            }
        }
    }
}

fn format_decimal(value: f64, unit: &str) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0} {}", value.round(), unit)
    } else {
        format!("{value:.1} {unit}")
    }
}

fn render_statistics(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let reading = &app.dashboard.reading;
    let rows = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Min(4),
    ])
    .spacing(1)
    .split(area);
    let top = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(rows[0]);
    let metrics = [
        (
            "STREAK",
            format!("{} days", reading.current_streak),
            theme.warning,
        ),
        (
            "THIS MONTH",
            reading.monthly_reading.to_string(),
            theme.accent,
        ),
        (
            "THIS YEAR",
            reading.yearly_reading.to_string(),
            theme.success,
        ),
        ("SESSIONS", reading.sessions.to_string(), theme.secondary),
    ];
    for (area, (label, value, color)) in top.iter().zip(metrics) {
        render_metric(frame, *area, label, &value, color, theme);
    }
    let middle = Layout::horizontal([Constraint::Ratio(1, 3); 3])
        .spacing(1)
        .split(rows[1]);
    render_metric(
        frame,
        middle[0],
        "MOST ACTIVE DAY",
        reading.most_active_day.as_deref().unwrap_or("No data"),
        theme.accent,
        theme,
    );
    render_metric(
        frame,
        middle[1],
        "MOST READ AUTHOR",
        reading.most_read_author.as_deref().unwrap_or("No data"),
        theme.secondary,
        theme,
    );
    render_metric(
        frame,
        middle[2],
        "MOST READ JOURNAL",
        reading.most_read_journal.as_deref().unwrap_or("No data"),
        theme.success,
        theme,
    );
    let heatmap = reading
        .heatmap
        .iter()
        .map(|day| match day.count {
            0 => ' ',
            1 => '░',
            2..=3 => '▒',
            4..=6 => '▓',
            _ => '█',
        })
        .collect::<String>();
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(heatmap, Style::default().fg(theme.success)),
            Line::raw(""),
            Line::styled(
                format!(
                    "Average reading time: {}",
                    format_duration(reading.average_reading_seconds)
                ),
                Style::default().fg(theme.muted),
            ),
            Line::styled(
                format!(
                    "Total reading duration: {}",
                    format_duration(reading.total_reading_seconds)
                ),
                Style::default().fg(theme.muted),
            ),
        ])
        .block(
            Block::default()
                .title(" 12-WEEK READING ACTIVITY ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        ),
        rows[2],
    );
}

fn render_metric(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    value: &str,
    color: ratatui::style::Color,
    theme: &Theme,
) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(label, Style::default().fg(theme.muted)),
            Line::styled(
                value,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        ),
        area,
    );
}

fn activity_kind(kind: &str) -> &str {
    match kind {
        "paper_browsed" => "Paper browsed",
        "paper_opened" | "pdf_opened" => "Paper opened",
        "note_opened" => "Note opened",
        "search" => "Search",
        "downloaded" => "Downloaded",
        "bookmarked" => "Bookmark changed",
        "tagged" => "Legacy organization event",
        "collected" => "Added to group",
        "project_opened" => "Worked on",
        "project_created" => "Project created",
        "project_renamed" => "Project renamed",
        "project_deleted" => "Project deleted",
        "paper_created" => "Paper created",
        "paper_renamed" => "Paper renamed",
        "paper_deleted" => "Paper deleted",
        _ => kind,
    }
}

fn render_organization(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let available_width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem<'_>> = match app.page {
        Page::Bookmarks => app
            .filtered_bookmarks()
            .iter()
            .map(|item| {
                let lib_paper = app.library.papers.iter().find(|p| p.id == item.paper_id);
                let reading_status = lib_paper.map_or("", |p| p.reading_status.as_str());
                let file_size = lib_paper.and_then(|p| p.file_size);
                let lines = build_paper_lines(
                    app,
                    theme,
                    PaperLineContext {
                        paper_id: Some(item.paper_id),
                        title: item.display_name(),
                        authors: &item.authors,
                        reading_status,
                        file_size,
                        availability: None,
                        bookmark_year: item.year.as_deref(),
                        bookmark_journal: item.journal.as_deref(),
                        bookmark_doi: item.doi.as_deref(),
                        bookmark_page: item.page.map(u64::from),
                        download_label: None,
                        available_width,
                    },
                );
                workspace_list_item(lines)
            })
            .collect(),
        Page::Notes => app
            .filtered_notes_papers()
            .iter()
            .map(|paper| {
                let lines = build_paper_lines(
                    app,
                    theme,
                    PaperLineContext {
                        paper_id: Some(paper.id),
                        title: paper.display_name(),
                        authors: &paper.authors,
                        reading_status: &paper.reading_status,
                        file_size: paper.file_size,
                        availability: None,
                        bookmark_year: None,
                        bookmark_journal: None,
                        bookmark_doi: None,
                        bookmark_page: None,
                        download_label: None,
                        available_width,
                    },
                );
                workspace_list_item(lines)
            })
            .collect(),
        _ => Vec::new(),
    };
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(format!("No {} yet", app.page.title().to_ascii_lowercase()))
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
    } else {
        let (title, selected_idx, scroll_idx) = match app.page {
            Page::Bookmarks => (
                " BOOKMARKS - j/k SELECT  ENTER/p OPEN  B REMOVE ",
                app.bookmark_selected,
                app.bookmark_scroll,
            ),
            Page::Notes => (
                " NOTES - j/k SELECT  ENTER/n EDIT  x DELETE ",
                app.notes_selected,
                app.notes_scroll,
            ),
            _ => ("", 0, 0),
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.border)),
            )
            .style(Style::default().fg(theme.text))
            .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
            .highlight_symbol("> ");
        let mut state = ListState::default()
            .with_selected(workspace_highlight_selection(app, selected_idx))
            .with_offset(scroll_idx);
        frame.render_stateful_widget(list, area, &mut state);
        match app.page {
            Page::Bookmarks => app.bookmark_scroll = state.offset(),
            Page::Notes => app.notes_scroll = state.offset(),
            _ => {}
        }
    }
}

fn render_note_editor(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let body = app
        .note_editor
        .as_ref()
        .map_or("", |note| note.body.as_str());
    let preview = app
        .overlay_flags
        .note_preview
        .then(|| markdown_preview(body, theme));
    let content = if let Some(preview) = &preview {
        preview.lines.clone()
    } else {
        body.split('\n').map(|l| Line::raw(l.to_string())).collect()
    };
    let min_height = frame.area().height.saturating_sub(6).clamp(16, 32);
    let desired_height = u16::try_from(content.len().saturating_add(2)).unwrap_or(u16::MAX);
    let area = centered(
        frame.area().width.saturating_sub(4),
        desired_height.max(min_height),
        frame.area(),
    );
    let visible_rows = area.height.saturating_sub(2);
    if !app.overlay_flags.note_preview {
        let cursor = app.note_editor.as_ref().map_or(0, |note| note.cursor);
        let cursor_row =
            u16::try_from(body[..cursor].lines().count().saturating_sub(1)).unwrap_or(0);
        if cursor_row < app.note_scroll {
            app.note_scroll = cursor_row;
        } else if cursor_row >= app.note_scroll.saturating_add(visible_rows) {
            app.note_scroll = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
        }
    }
    frame.render_widget(Clear, area);
    let title = if app.overlay_flags.note_preview {
        " MARKDOWN PREVIEW - TAB TO EDIT - j/k SCROLL - CLICK LINKS "
    } else {
        " MARKDOWN NOTE - AUTOSAVED - TAB TO PREVIEW "
    };
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .wrap(Wrap { trim: false })
            .scroll((app.note_scroll, 0))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent)),
            ),
        area,
    );
    if let Some(preview) = preview {
        frame.render_widget(
            MarkdownHyperlinks::new(preview.hyperlinks, app.note_scroll),
            Rect::new(
                area.x.saturating_add(1),
                area.y.saturating_add(1),
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            ),
        );
    }
    if app.overlay_flags.note_preview {
        return;
    }
    let cursor = app.note_editor.as_ref().map_or(0, |note| note.cursor);
    let text_before_cursor = &body[..cursor];
    let row = u16::try_from(text_before_cursor.split('\n').count().saturating_sub(1)).unwrap_or(0);
    let column = u16::try_from(
        text_before_cursor
            .split('\n')
            .next_back()
            .unwrap_or("")
            .chars()
            .count(),
    )
    .unwrap_or(0);
    let cursor_x = area.x.saturating_add(1).saturating_add(column);
    let cursor_y = area
        .y
        .saturating_add(1)
        .saturating_add(row.saturating_sub(app.note_scroll));
    frame.set_cursor_position((
        cursor_x.min(area.right().saturating_sub(2)),
        cursor_y.min(area.bottom().saturating_sub(2)),
    ));
}

#[derive(Default)]
struct MarkdownTable {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    in_cell: bool,
}

#[derive(Clone)]
struct MarkdownLink {
    line: usize,
    column: u16,
    width: u16,
    destination: String,
}

struct MarkdownPreview {
    lines: Vec<Line<'static>>,
    hyperlinks: Vec<MarkdownLink>,
}

struct MarkdownHyperlinks {
    links: Vec<MarkdownLink>,
    scroll: u16,
}

impl MarkdownHyperlinks {
    fn new(links: Vec<MarkdownLink>, scroll: u16) -> Self {
        Self { links, scroll }
    }
}

impl Widget for MarkdownHyperlinks {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for link in self.links {
            let line = u16::try_from(link.line).unwrap_or(u16::MAX);
            let Some(y) = line.checked_sub(self.scroll) else {
                continue;
            };
            if y >= area.height || link.column.saturating_add(link.width) > area.width {
                continue;
            }
            let safe_url = link.destination.replace(['\x1b', '\x07'], "");
            // Ratatui calculates ANSI escape-sequence widths correctly only when each OSC 8
            // hyperlink is written in two-cell chunks (ratatui#902).
            let end = link.column.saturating_add(link.width);
            let mut x = link.column;
            while x < end {
                let first = buffer[(area.x.saturating_add(x), area.y.saturating_add(y))]
                    .symbol()
                    .to_owned();
                if first.is_empty() {
                    x = x.saturating_add(2);
                    continue;
                }
                let second_x = x.saturating_add(1);
                let second = if second_x < end {
                    buffer[(area.x.saturating_add(second_x), area.y.saturating_add(y))].symbol()
                } else {
                    ""
                };
                let hyperlink = format!("\x1b]8;;{safe_url}\x07{first}{second}\x1b]8;;\x07");
                buffer[(area.x.saturating_add(x), area.y.saturating_add(y))].set_symbol(&hyperlink);
                x = x.saturating_add(2);
            }
        }
    }
}

struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    link_destinations: Vec<(String, usize, u16)>,
    hyperlinks: Vec<MarkdownLink>,
    image_destinations: Vec<String>,
    code_block: Option<(String, String)>,
    table: Option<MarkdownTable>,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default().fg(theme.text)],
            lists: Vec::new(),
            quote_depth: 0,
            link_destinations: Vec::new(),
            hyperlinks: Vec::new(),
            image_destinations: Vec::new(),
            code_block: None,
            table: None,
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push(&mut self, text: impl Into<String>, style: Style) {
        let text = text.into();
        if let Some(table) = &mut self.table
            && table.in_cell
        {
            table.cell.push_str(&text);
            return;
        }
        self.current.push(Span::styled(text, style));
    }

    fn text(&mut self, text: &str) {
        if let Some((_, code)) = &mut self.code_block {
            code.push_str(text);
        } else {
            self.push(text.to_owned(), self.style());
        }
    }

    fn finish_line(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn current_width(&self) -> u16 {
        self.current.iter().fold(0_u16, |width, span| {
            width.saturating_add(u16::try_from(span.content.chars().count()).unwrap_or(u16::MAX))
        })
    }

    fn block_prefix(&mut self) {
        if self.quote_depth > 0 {
            self.push(
                "│ ".repeat(self.quote_depth),
                Style::default().fg(self.theme.warning),
            );
        }
    }

    fn start_item(&mut self) {
        self.finish_line();
        self.block_prefix();
        let indent = "  ".repeat(self.lists.len().saturating_sub(1));
        let marker = match self.lists.last_mut() {
            Some(Some(number)) => {
                let marker = format!("{number}. ");
                *number += 1;
                marker
            }
            _ => "• ".to_owned(),
        };
        self.push(indent, Style::default().fg(self.theme.secondary));
        self.push(marker, Style::default().fg(self.theme.secondary));
    }

    fn finish_code_block(&mut self) {
        let Some((language, source)) = self.code_block.take() else {
            return;
        };
        self.finish_line();
        let label = if language.is_empty() {
            "plain text"
        } else {
            &language
        };
        self.lines.push(Line::styled(
            format!("── Code ({label}) ─────────────────────────────"),
            Style::default().fg(self.theme.muted),
        ));
        let (syntax_set, theme_set) = markdown_syntax_assets();
        let syntax = syntax_set
            .find_syntax_by_token(&language)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
        let syntax_theme = theme_set.themes.get("base16-ocean.dark");
        if let Some(syntax_theme) = syntax_theme {
            let mut highlighter = HighlightLines::new(syntax, syntax_theme);
            for line in LinesWithEndings::from(&source) {
                let spans: Vec<Span<'static>> = highlighter
                    .highlight_line(line.trim_end_matches(['\r', '\n']), syntax_set)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_owned(), syntect_style(style.foreground, self.theme))
                    })
                    .collect();
                self.lines.push(Line::from(spans));
            }
        } else {
            for line in source.lines() {
                self.lines.push(Line::styled(
                    line.to_owned(),
                    Style::default().fg(self.theme.success),
                ));
            }
        }
    }

    fn finish_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        if table.in_cell {
            table.row.push(std::mem::take(&mut table.cell));
        }
        if !table.row.is_empty() {
            table.rows.push(table.row);
        }
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![3; columns];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
        for (row_index, row) in table.rows.iter().enumerate() {
            let mut rendered = String::from("│");
            for (index, width) in widths.iter().enumerate() {
                let cell = row.get(index).map_or("", String::as_str);
                let _ = write!(rendered, " {cell:width$} │", width = *width);
            }
            self.lines.push(Line::styled(
                rendered,
                Style::default()
                    .fg(if row_index == 0 {
                        self.theme.accent
                    } else {
                        self.theme.text
                    })
                    .add_modifier(if row_index == 0 {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ));
            if row_index == 0 {
                self.lines.push(Line::styled(
                    format!(
                        "├{}┤",
                        widths
                            .iter()
                            .map(|width| "─".repeat(width + 2))
                            .collect::<Vec<_>>()
                            .join("┼")
                    ),
                    Style::default().fg(self.theme.border),
                ));
            }
        }
    }
}

fn syntect_style(color: SyntectColor, theme: &Theme) -> Style {
    // Some grammars use the default foreground; keep it legible on every Papr theme.
    let foreground = if color.a == 0 {
        theme.text
    } else {
        ratatui::style::Color::Rgb(color.r, color.g, color.b)
    };
    Style::default().fg(foreground)
}

fn preserve_source_spacing(renderer: &mut MarkdownRenderer<'_>, body: &str, start: usize) {
    if !renderer.current.is_empty() || renderer.lines.is_empty() {
        return;
    }
    let blank_lines = body[..start]
        .lines()
        .rev()
        .take_while(|line| line.trim().is_empty())
        .count();
    renderer
        .lines
        .extend(std::iter::repeat_n(Line::raw(""), blank_lines));
}

fn markdown_preview(body: &str, theme: &Theme) -> MarkdownPreview {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let mut renderer = MarkdownRenderer::new(theme);

    for (event, range) in Parser::new_ext(body, options).into_offset_iter() {
        match event {
            Event::Start(tag) => start_markdown_tag(&mut renderer, tag, body, range.start, theme),
            Event::End(tag) => end_markdown_tag(&mut renderer, tag, theme),
            Event::TaskListMarker(done) => renderer.push(
                if done { "[x] " } else { "[ ] " },
                Style::default().fg(if done { theme.success } else { theme.warning }),
            ),
            Event::Code(code) => renderer.push(
                code.to_string(),
                renderer.style().fg(theme.success).bg(theme.surface),
            ),
            Event::InlineMath(math) => renderer.push(
                format_math(&math),
                renderer
                    .style()
                    .fg(theme.warning)
                    .add_modifier(Modifier::ITALIC),
            ),
            Event::DisplayMath(math) => {
                renderer.finish_line();
                renderer.lines.push(Line::styled(
                    format!("  {}", format_math(&math)),
                    Style::default()
                        .fg(theme.warning)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            Event::FootnoteReference(label) => {
                renderer.push(format!("[^{label}]"), Style::default().fg(theme.accent));
            }
            Event::SoftBreak | Event::HardBreak => renderer.finish_line(),
            Event::Rule => {
                renderer.finish_line();
                renderer.lines.push(Line::styled(
                    "────────────────────────────────────────────────",
                    Style::default().fg(theme.border),
                ));
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => renderer.text(&text),
        }
    }
    renderer.finish_code_block();
    renderer.finish_table();
    renderer.finish_line();
    MarkdownPreview {
        lines: renderer.lines,
        hyperlinks: renderer.hyperlinks,
    }
}

fn start_markdown_tag(
    renderer: &mut MarkdownRenderer<'_>,
    tag: Tag<'_>,
    body: &str,
    start: usize,
    theme: &Theme,
) {
    match tag {
        Tag::Paragraph => {
            preserve_source_spacing(renderer, body, start);
            renderer.block_prefix();
        }
        Tag::Heading { level, .. } => {
            renderer.finish_line();
            preserve_source_spacing(renderer, body, start);
            renderer.block_prefix();
            renderer.styles.push(
                renderer
                    .style()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            );
            renderer.push(
                format!("{} ", "━".repeat(level as usize + 1)),
                renderer.style(),
            );
        }
        Tag::Emphasis => renderer
            .styles
            .push(renderer.style().add_modifier(Modifier::ITALIC)),
        Tag::Strong => renderer
            .styles
            .push(renderer.style().add_modifier(Modifier::BOLD)),
        Tag::Strikethrough => renderer
            .styles
            .push(renderer.style().add_modifier(Modifier::CROSSED_OUT)),
        Tag::Link { dest_url, .. } => {
            renderer.link_destinations.push((
                dest_url.to_string(),
                renderer.lines.len(),
                renderer.current_width(),
            ));
            renderer.styles.push(
                renderer
                    .style()
                    .fg(theme.accent)
                    .add_modifier(Modifier::UNDERLINED),
            );
        }
        Tag::Image { dest_url, .. } => {
            renderer.image_destinations.push(dest_url.to_string());
            renderer.push("🖼 ", Style::default().fg(theme.secondary));
            renderer.styles.push(
                renderer
                    .style()
                    .fg(theme.accent)
                    .add_modifier(Modifier::UNDERLINED),
            );
        }
        Tag::BlockQuote(_) => {
            renderer.finish_line();
            preserve_source_spacing(renderer, body, start);
            renderer.quote_depth += 1;
        }
        Tag::List(first) => {
            renderer.finish_line();
            preserve_source_spacing(renderer, body, start);
            renderer.lists.push(first);
        }
        Tag::Item => renderer.start_item(),
        Tag::CodeBlock(kind) => {
            renderer.finish_line();
            preserve_source_spacing(renderer, body, start);
            let language = match kind {
                CodeBlockKind::Fenced(language) => language.to_string(),
                CodeBlockKind::Indented => String::new(),
            };
            renderer.code_block = Some((language, String::new()));
        }
        Tag::FootnoteDefinition(label) => {
            renderer.finish_line();
            renderer.push(format!("[^{label}]: "), Style::default().fg(theme.accent));
        }
        Tag::Table(_) => {
            renderer.finish_line();
            preserve_source_spacing(renderer, body, start);
            renderer.table = Some(MarkdownTable::default());
        }
        Tag::TableRow => {
            if let Some(table) = &mut renderer.table
                && !table.row.is_empty()
            {
                table.rows.push(std::mem::take(&mut table.row));
            }
        }
        Tag::TableCell => {
            if let Some(table) = &mut renderer.table {
                table.in_cell = true;
            }
        }
        _ => {}
    }
}

fn end_markdown_tag(renderer: &mut MarkdownRenderer<'_>, tag: TagEnd, theme: &Theme) {
    match tag {
        TagEnd::Heading(_) => {
            renderer.styles.pop();
            renderer.finish_line();
        }
        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
            renderer.styles.pop();
        }
        TagEnd::Link => {
            renderer.styles.pop();
            if let Some((destination, line, column)) = renderer.link_destinations.pop() {
                let width = renderer.current_width().saturating_sub(column);
                if width > 0 && line == renderer.lines.len() {
                    renderer.hyperlinks.push(MarkdownLink {
                        line,
                        column,
                        width,
                        destination,
                    });
                }
            }
        }
        TagEnd::Image => {
            renderer.styles.pop();
            if let Some(destination) = renderer.image_destinations.pop() {
                renderer.push(
                    format!(" ({destination})"),
                    Style::default().fg(theme.muted),
                );
            }
        }
        TagEnd::BlockQuote(_) => {
            renderer.finish_line();
            renderer.quote_depth = renderer.quote_depth.saturating_sub(1);
        }
        TagEnd::List(_) => {
            renderer.finish_line();
            renderer.lists.pop();
        }
        TagEnd::CodeBlock => renderer.finish_code_block(),
        TagEnd::Table => renderer.finish_table(),
        TagEnd::TableCell => {
            if let Some(table) = &mut renderer.table {
                table.row.push(std::mem::take(&mut table.cell));
                table.in_cell = false;
            }
        }
        TagEnd::TableRow => {
            if let Some(table) = &mut renderer.table {
                table.rows.push(std::mem::take(&mut table.row));
            }
        }
        TagEnd::Paragraph | TagEnd::FootnoteDefinition | TagEnd::Item => renderer.finish_line(),
        _ => {}
    }
}

/// A small, recursive TeX math renderer for terminal previews.
///
/// Markdown parsing is delegated to `pulldown-cmark`; this renderer receives only
/// math tokens. It deliberately understands TeX structure (groups, scripts,
/// commands, and environments) instead of applying command-by-command string
/// replacements. Unsupported commands are retained verbatim, and a malformed
/// expression falls back to its original source.
struct LatexRenderer {
    input: Vec<char>,
    position: usize,
}

impl LatexRenderer {
    fn new(source: &str) -> Self {
        Self {
            input: source.chars().collect(),
            position: 0,
        }
    }

    fn render(mut self) -> Result<String, ()> {
        let rendered = self.expression(None)?;
        (self.position == self.input.len())
            .then_some(rendered)
            .ok_or(())
    }

    fn expression(&mut self, stop: Option<char>) -> Result<String, ()> {
        let mut output = String::new();
        while let Some(&character) = self.input.get(self.position) {
            if Some(character) == stop {
                self.position += 1;
                return Ok(output);
            }
            if character == '}' {
                return Err(());
            }
            self.position += 1;
            match character {
                '{' => output.push_str(&self.expression(Some('}'))?),
                '^' => output.push_str(&superscript(&self.atom()?)),
                '_' => output.push_str(&subscript(&self.atom()?)),
                '\\' => output.push_str(&self.command()?),
                '~' | '&' => output.push(' '),
                character => output.push(character),
            }
        }
        stop.is_none().then_some(output).ok_or(())
    }

    fn atom(&mut self) -> Result<String, ()> {
        let character = *self.input.get(self.position).ok_or(())?;
        self.position += 1;
        match character {
            '{' => self.expression(Some('}')),
            '\\' => self.command(),
            '^' => Ok(superscript(&self.atom()?)),
            '_' => Ok(subscript(&self.atom()?)),
            '}' => Err(()),
            character => Ok(character.to_string()),
        }
    }

    fn group(&mut self) -> Result<String, ()> {
        if self.input.get(self.position) != Some(&'{') {
            return Err(());
        }
        self.position += 1;
        self.expression(Some('}'))
    }

    fn optional_group(&mut self) -> Result<Option<String>, ()> {
        if self.input.get(self.position) != Some(&'[') {
            return Ok(None);
        }
        self.position += 1;
        let mut depth = 1_usize;
        let start = self.position;
        while let Some(&character) = self.input.get(self.position) {
            self.position += 1;
            match character {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Some(self.input[start..self.position - 1].iter().collect()));
                    }
                }
                _ => {}
            }
        }
        Err(())
    }

    fn command(&mut self) -> Result<String, ()> {
        let mut name = String::new();
        while let Some(character) = self.input.get(self.position).copied() {
            if !character.is_ascii_alphabetic() {
                break;
            }
            self.position += 1;
            name.push(character);
        }
        if name.is_empty() {
            let escaped = self.input.get(self.position).copied().ok_or(())?;
            self.position += 1;
            return Ok(match escaped {
                '\\' | ' ' | ',' | ';' | ':' | '!' => " ".to_owned(),
                '#' | '$' | '%' | '&' | '_' | '{' | '}' | '[' | ']' | '^' | '~' => {
                    escaped.to_string()
                }
                _ => format!("\\{escaped}"),
            });
        }

        if let Some(symbol) = latex_symbol(&name) {
            return Ok(symbol.to_owned());
        }
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                let numerator = self.group()?;
                let denominator = self.group()?;
                Ok(if name.ends_with("binom") || name == "binom" {
                    format!("({numerator} choose {denominator})")
                } else {
                    format!("({numerator})⁄({denominator})")
                })
            }
            "sqrt" => {
                let degree = self.optional_group()?;
                let radicand = self.group()?;
                Ok(degree.map_or_else(
                    || format!("√({radicand})"),
                    |degree| format!("{}√({radicand})", superscript(&degree)),
                ))
            }
            "text"
            | "textrm"
            | "textbf"
            | "textit"
            | "texttt"
            | "mathrm"
            | "mathbf"
            | "mathcal"
            | "mathfrak"
            | "mathsf"
            | "mathtt"
            | "mathit"
            | "boldsymbol"
            | "bm"
            | "operatorname"
            | "operatornamewithlimits" => {
                if name == "operatorname" && self.input.get(self.position) == Some(&'*') {
                    self.position += 1;
                }
                self.group()
            }
            "mathbb" => Ok(mathbb(&self.group()?)),
            "overline" | "bar" => Ok(format!("{}\u{0305}", self.group()?)),
            "underline" => Ok(format!("_{}", self.group()?)),
            "hat" | "widehat" => Ok(format!("{}\u{0302}", self.group()?)),
            "tilde" | "widetilde" => Ok(format!("{}\u{0303}", self.group()?)),
            "vec" => Ok(format!("{}\u{20d7}", self.group()?)),
            "dot" => Ok(format!("{}\u{0307}", self.group()?)),
            "ddot" => Ok(format!("{}\u{0308}", self.group()?)),
            "left" | "right" | "middle" => self.delimiter(),
            "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "textstyle" | "scriptstyle"
            | "scriptscriptstyle" | "limits" | "nolimits" | "qquad" | "quad" | "enspace"
            | "hspace" | "vspace" => {
                if matches!(name.as_str(), "hspace" | "vspace") {
                    let _ = self.group()?;
                }
                Ok(if matches!(name.as_str(), "qquad" | "quad" | "enspace") {
                    " ".to_owned()
                } else {
                    String::new()
                })
            }
            "begin" => self.environment(),
            "end" => Err(()),
            _ => Ok(format!("\\{name}")),
        }
    }

    fn delimiter(&mut self) -> Result<String, ()> {
        let character = self.input.get(self.position).copied().ok_or(())?;
        self.position += 1;
        if character != '\\' {
            return Ok(match character {
                '.' => String::new(),
                '{' => "{".to_owned(),
                '}' => "}".to_owned(),
                _ => character.to_string(),
            });
        }
        let mut name = String::new();
        while let Some(character) = self.input.get(self.position).copied() {
            if !character.is_ascii_alphabetic() {
                break;
            }
            self.position += 1;
            name.push(character);
        }
        if name.is_empty() {
            let escaped = self.input.get(self.position).copied().ok_or(())?;
            self.position += 1;
            return Ok(match escaped {
                '{' => "{".to_owned(),
                '}' => "}".to_owned(),
                '|' => "|".to_owned(),
                _ => format!("\\{escaped}"),
            });
        }
        Ok(match name.as_str() {
            "lbrace" | "rbrace" => if name.starts_with('l') { "{" } else { "}" }.to_owned(),
            "langle" | "rangle" => if name.starts_with('l') { "⟨" } else { "⟩" }.to_owned(),
            "lceil" | "rceil" => if name.starts_with('l') { "⌈" } else { "⌉" }.to_owned(),
            "lfloor" | "rfloor" => if name.starts_with('l') { "⌊" } else { "⌋" }.to_owned(),
            "vert" | "mid" | "lvert" | "rvert" => "|".to_owned(),
            "Vert" | "lVert" | "rVert" => "‖".to_owned(),
            _ => format!("\\{name}"),
        })
    }

    fn environment(&mut self) -> Result<String, ()> {
        let name = self.group()?;
        let end_marker = format!("\\end{{{name}}}");
        let remaining: String = self.input[self.position..].iter().collect();
        let end = remaining.find(&end_marker).ok_or(())?;
        let body = &remaining[..end];
        self.position += remaining[..end + end_marker.len()].chars().count();
        let rows = body
            .split("\\\\")
            .map(|row| {
                row.split('&')
                    .map(|cell| LatexRenderer::new(cell.trim()).render())
                    .collect::<Result<Vec<_>, _>>()
                    .map(|cells| cells.join("  "))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" ; ");
        let (left, right) = match name.as_str() {
            "pmatrix" => ("(", ")"),
            "bmatrix" => ("[", "]"),
            "Bmatrix" => ("{", "}"),
            "vmatrix" => ("|", "|"),
            "Vmatrix" => ("‖", "‖"),
            "cases" => ("{", ""),
            _ => ("", ""),
        };
        Ok(format!("{left}{rows}{right}"))
    }
}

fn latex_symbol(name: &str) -> Option<&'static str> {
    latex_greek_symbol(name).or_else(|| latex_operator_symbol(name))
}

fn latex_greek_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" | "vartheta" => "θ",
        "iota" => "ι",
        "kappa" | "varkappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" | "varpi" => "π",
        "rho" | "varrho" => "ρ",
        "sigma" | "varsigma" => "σ",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        _ => return None,
    })
}

fn latex_operator_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "sum" => "∑",
        "prod" | "coprod" => "∏",
        "int" | "iint" | "iiint" | "oint" => "∫",
        "lim" => "lim",
        "limsup" => "lim sup",
        "liminf" => "lim inf",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "forall" => "∀",
        "exists" => "∃",
        "neg" => "¬",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "to" | "rightarrow" | "longrightarrow" | "mapsto" => "→",
        "leftarrow" | "longleftarrow" => "←",
        "leftrightarrow" | "iff" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" => "⇔",
        "times" => "×",
        "cdot" | "bullet" => "·",
        "ast" => "∗",
        "circ" => "∘",
        "pm" => "±",
        "mp" => "∓",
        "div" => "÷",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "cup" => "∪",
        "cap" => "∩",
        "le" | "leq" | "leqslant" => "≤",
        "ge" | "geq" | "geqslant" => "≥",
        "ne" | "neq" => "≠",
        "equiv" => "≡",
        "approx" => "≈",
        "sim" => "∼",
        "propto" => "∝",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "perp" => "⊥",
        "parallel" => "∥",
        "angle" => "∠",
        "degree" => "°",
        "prime" => "′",
        "ldots" | "cdots" | "dots" => "…",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "cot" => "cot",
        "sec" => "sec",
        "csc" => "csc",
        "log" => "log",
        "ln" => "ln",
        "exp" => "exp",
        "max" => "max",
        "min" => "min",
        "sup" => "sup",
        "inf" => "inf",
        "det" => "det",
        "gcd" => "gcd",
        "Pr" => "Pr",
        _ => return None,
    })
}

fn mathbb(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'C' => 'ℂ',
            'H' => 'ℍ',
            'N' => 'ℕ',
            'P' => 'ℙ',
            'Q' => 'ℚ',
            'R' => 'ℝ',
            'Z' => 'ℤ',
            character => character,
        })
        .collect()
}

fn superscript(value: &str) -> String {
    script(value, true)
}

fn subscript(value: &str) -> String {
    script(value, false)
}

fn script(value: &str, upper: bool) -> String {
    let compact = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let converted: Option<String> = compact
        .chars()
        .map(|character| match (upper, character) {
            (true, '0') => Some('⁰'),
            (true, '1') => Some('¹'),
            (true, '2') => Some('²'),
            (true, '3') => Some('³'),
            (true, '4') => Some('⁴'),
            (true, '5') => Some('⁵'),
            (true, '6') => Some('⁶'),
            (true, '7') => Some('⁷'),
            (true, '8') => Some('⁸'),
            (true, '9') => Some('⁹'),
            (true, '+') => Some('⁺'),
            (true, '-') => Some('⁻'),
            (true, '=') => Some('⁼'),
            (true, '(') => Some('⁽'),
            (true, ')') => Some('⁾'),
            (true, 'a') => Some('ᵃ'),
            (true, 'b') => Some('ᵇ'),
            (true, 'c') => Some('ᶜ'),
            (true, 'd') => Some('ᵈ'),
            (true, 'e') => Some('ᵉ'),
            (true, 'f') => Some('ᶠ'),
            (true, 'g') => Some('ᵍ'),
            (true, 'h') => Some('ʰ'),
            (true, 'i') => Some('ⁱ'),
            (true, 'j') => Some('ʲ'),
            (true, 'k') => Some('ᵏ'),
            (true, 'l') => Some('ˡ'),
            (true, 'm') => Some('ᵐ'),
            (true, 'n') => Some('ⁿ'),
            (true, 'o') => Some('ᵒ'),
            (true, 'p') => Some('ᵖ'),
            (true, 'r') => Some('ʳ'),
            (true, 's') => Some('ˢ'),
            (true, 't') => Some('ᵗ'),
            (true, 'u') => Some('ᵘ'),
            (true, 'v') => Some('ᵛ'),
            (true, 'w') => Some('ʷ'),
            (true, 'x') => Some('ˣ'),
            (true, 'y') => Some('ʸ'),
            (true, 'z') => Some('ᶻ'),
            (false, '0') => Some('₀'),
            (false, '1') => Some('₁'),
            (false, '2') => Some('₂'),
            (false, '3') => Some('₃'),
            (false, '4') => Some('₄'),
            (false, '5') => Some('₅'),
            (false, '6') => Some('₆'),
            (false, '7') => Some('₇'),
            (false, '8') => Some('₈'),
            (false, '9') => Some('₉'),
            (false, '+') => Some('₊'),
            (false, '-') => Some('₋'),
            (false, '=') => Some('₌'),
            (false, '(') => Some('₍'),
            (false, ')') => Some('₎'),
            (false, 'a') => Some('ₐ'),
            (false, 'e') => Some('ₑ'),
            (false, 'h') => Some('ₕ'),
            (false, 'i') => Some('ᵢ'),
            (false, 'j') => Some('ⱼ'),
            (false, 'k') => Some('ₖ'),
            (false, 'l') => Some('ₗ'),
            (false, 'm') => Some('ₘ'),
            (false, 'n') => Some('ₙ'),
            (false, 'o') => Some('ₒ'),
            (false, 'p') => Some('ₚ'),
            (false, 'r') => Some('ᵣ'),
            (false, 's') => Some('ₛ'),
            (false, 't') => Some('ₜ'),
            (false, 'u') => Some('ᵤ'),
            (false, 'v') => Some('ᵥ'),
            (false, 'x') => Some('ₓ'),
            _ => None,
        })
        .collect();
    converted.map_or_else(
        || {
            if upper {
                format!("⁽{compact}⁾")
            } else {
                format!("₍{compact}₎")
            }
        },
        String::from,
    )
}

fn format_math(source: &str) -> String {
    LatexRenderer::new(source)
        .render()
        .unwrap_or_else(|()| source.to_owned())
}

fn chunk_string(s: &str, size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    for c in s.chars() {
        if current_chunk.chars().count() == size {
            chunks.push(current_chunk);
            current_chunk = String::new();
        }
        current_chunk.push(c);
    }
    if !current_chunk.is_empty() || chunks.is_empty() {
        chunks.push(current_chunk);
    }
    chunks
}

fn render_delete_confirmation(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(target) = &app.delete_confirmation else {
        return;
    };

    let (title, message, item_name) = match target {
        DeletionTarget::Project { project } => (
            " CONFIRM DELETE PROJECT ",
            "Are you sure you want to permanently delete this project directory and all of its files?",
            project.name.as_str(),
        ),
        DeletionTarget::Paper { title, .. } => (
            " CONFIRM DELETE PDF ",
            "Are you sure you want to permanently delete this PDF file from your disk?",
            title.as_str(),
        ),
        DeletionTarget::Collection { name, .. } => (
            " CONFIRM DELETE GROUP ",
            "Are you sure you want to permanently delete this group (subdirectory) and ALL of its contents?",
            name.as_str(),
        ),
        DeletionTarget::ProjectEntry {
            name, is_directory, ..
        } => (
            if *is_directory {
                " CONFIRM DELETE FOLDER "
            } else {
                " CONFIRM DELETE FILE "
            },
            if *is_directory {
                "Are you sure you want to permanently delete this folder and all of its contents?"
            } else {
                "Are you sure you want to permanently delete this file?"
            },
            name.as_str(),
        ),
    };

    let height = 12;
    let width = 64;
    let area = centered(width, height, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface)),
        area,
    );

    let lines = vec![
        Line::raw(""),
        Line::styled(message, Style::default().fg(theme.text)),
        Line::raw(""),
        Line::styled(
            format!("  \"{item_name}\""),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(
            "Press [y/Enter] to confirm, or [n/Esc/q] to cancel.",
            Style::default().fg(theme.muted),
        ),
    ];

    let block = Block::default()
        .title(Line::styled(
            title,
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error))
        .style(Style::default().bg(theme.surface));

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_metadata_prompt(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let Some(prompt) = &app.metadata_prompt else {
        return;
    };
    let renaming_pdf = prompt.rename_paper_id.is_some();
    let renaming = prompt.rename_collection_id.is_some();
    let creating = prompt.paper_id.is_none() && !renaming && !renaming_pdf;
    let title = if renaming_pdf {
        " RENAME PDF "
    } else if renaming {
        " RENAME GROUP "
    } else if creating {
        " CREATE GROUP "
    } else {
        " CHOOSE OR CREATE GROUP "
    };
    let prefix = if renaming || creating || renaming_pdf {
        "> "
    } else {
        "New name: "
    };
    let text = format!("{}{}", prefix, prompt.value);
    let chunks = chunk_string(&text, 62);
    let text_lines = if chunks.is_empty() {
        1
    } else {
        saturating_u16(chunks.len())
    };

    let has_current =
        prompt.current_collection.is_some() && !renaming && !creating && !renaming_pdf;
    let extra_rows = if has_current { 2 } else { 0 };

    let height = if renaming || creating || renaming_pdf {
        text_lines.max(1) + 2
    } else {
        text_lines.max(1) + 10 + extra_rows
    };

    let area = centered(64, height, frame.area());
    frame.render_widget(Clear, area);

    let mut lines = Vec::new();
    if let Some(current_collection) = prompt.current_collection.as_ref().filter(|_| has_current) {
        lines.push(Line::styled(
            format!("Current group: {current_collection}"),
            Style::default().fg(theme.muted),
        ));
        lines.push(Line::raw(""));
    }

    for chunk in &chunks {
        lines.push(Line::raw(chunk.clone()));
    }
    if chunks.is_empty() {
        lines.push(Line::raw(prefix));
    }

    if !renaming && !creating && !renaming_pdf {
        lines.push(Line::styled(
            "Or select an existing group:",
            Style::default().fg(theme.muted),
        ));
        let start = prompt.selected.saturating_sub(5);
        lines.extend(app.collections.iter().enumerate().skip(start).take(6).map(
            |(index, collection)| {
                Line::styled(
                    format!(
                        "{} {}",
                        if index == prompt.selected { ">" } else { " " },
                        collection.name
                    ),
                    Style::default().fg(if index == prompt.selected {
                        theme.accent
                    } else {
                        theme.text
                    }),
                )
            },
        ));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.secondary)),
            ),
        area,
    );

    let prefix_len = prefix.chars().count();
    let cursor_idx = prefix_len + prompt.value[..prompt.cursor].chars().count();
    let cursor_row = u16::try_from(cursor_idx / 62).unwrap_or(0) + extra_rows;
    let cursor_col = u16::try_from(cursor_idx % 62).unwrap_or(0);

    frame.set_cursor_position((
        area.x.saturating_add(1).saturating_add(cursor_col),
        area.y.saturating_add(1).saturating_add(cursor_row),
    ));
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    let papers = app.filtered_library_papers();
    let message = app
        .library
        .message
        .as_deref()
        .unwrap_or("Local papers from configured library folders. Press r to rescan.");
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(if app.library.indexing {
            theme.warning
        } else {
            theme.muted
        })),
        rows[0],
    );
    if papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No local papers indexed")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let available_width = rows[1].width.saturating_sub(2) as usize;
    let items = papers.iter().map(|paper| {
        let lines = build_paper_lines(
            app,
            theme,
            PaperLineContext {
                paper_id: Some(paper.id),
                title: paper.display_name(),
                authors: &paper.authors,
                reading_status: &paper.reading_status,
                file_size: paper.file_size,
                availability: None,
                bookmark_year: None,
                bookmark_journal: None,
                bookmark_doi: None,
                bookmark_page: None,
                download_label: None,
                available_width,
            },
        );
        workspace_list_item(lines)
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.library.selected))
        .with_offset(app.library.scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.library.scroll = state.offset();
}

fn render_reading_queue(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    let papers = app.filtered_reading_queue_papers();
    let message = "Prioritized reading queue. Use K/J to prioritize, a to dequeue, enter to read.";
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(theme.muted)),
        rows[0],
    );
    if papers.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No papers in reading queue. Press a on a paper in other workspaces to queue it.",
            )
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let available_width = rows[1].width.saturating_sub(2) as usize;
    let items = papers.iter().map(|paper| {
        let lines = build_paper_lines(
            app,
            theme,
            PaperLineContext {
                paper_id: Some(paper.id),
                title: paper.display_name(),
                authors: &paper.authors,
                reading_status: &paper.reading_status,
                file_size: paper.file_size,
                availability: None,
                bookmark_year: None,
                bookmark_journal: None,
                bookmark_doi: None,
                bookmark_page: None,
                download_label: None,
                available_width,
            },
        );
        workspace_list_item(lines)
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(
            app,
            app.reading_queue_selected,
        ))
        .with_offset(app.reading_queue_scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.reading_queue_scroll = state.offset();
}

fn render_downloads(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let downloads = app.filtered_downloads();
    if downloads.is_empty() {
        frame.render_widget(
            Paragraph::new("No downloads yet. Press d on an arXiv paper to start one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let available_width = area.width.saturating_sub(2) as usize;
    let items = downloads.iter().map(|download| {
        let (label, color) = download_status_label(download, theme);

        let paper = if let Some(paper_id) = download.paper_id {
            app.library.papers.iter().find(|p| p.id == paper_id)
        } else if let Some(pdf_path) = &download.pdf_path {
            app.library
                .papers
                .iter()
                .find(|p| p.pdf_path.as_ref() == Some(pdf_path))
        } else {
            None
        };

        let lines = if let Some(paper) = paper {
            let mut pl = build_paper_lines(
                app,
                theme,
                PaperLineContext {
                    paper_id: Some(paper.id),
                    title: paper.display_name(),
                    authors: &paper.authors,
                    reading_status: &paper.reading_status,
                    file_size: paper.file_size,
                    availability: None,
                    bookmark_year: None,
                    bookmark_journal: None,
                    bookmark_doi: None,
                    bookmark_page: None,
                    download_label: None,
                    available_width,
                },
            );
            let len = pl.len();
            pl.insert(len - 1, Line::styled(label, Style::default().fg(color)));
            pl
        } else {
            let mut pl = build_paper_lines(
                app,
                theme,
                PaperLineContext {
                    paper_id: None,
                    title: download.display_name(),
                    authors: "",
                    reading_status: "",
                    file_size: None,
                    availability: None,
                    bookmark_year: None,
                    bookmark_journal: None,
                    bookmark_doi: None,
                    bookmark_page: None,
                    download_label: None,
                    available_width,
                },
            );
            let len = pl.len();
            pl.insert(len - 1, Line::styled(label, Style::default().fg(color)));
            pl
        };

        workspace_list_item(lines)
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" DOWNLOAD MANAGER ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.download_selected))
        .with_offset(app.download_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.download_scroll = state.offset();
}

fn download_status_label(
    download: &DownloadTask,
    theme: &Theme,
) -> (String, ratatui::style::Color) {
    match &download.status {
        DownloadStatus::Starting => ("Starting".to_owned(), theme.warning),
        DownloadStatus::Running => (
            download.total.map_or_else(
                || format_bytes(download.downloaded),
                |total| {
                    format!(
                        "{} / {}",
                        format_bytes(download.downloaded),
                        format_bytes(total)
                    )
                },
            ),
            theme.accent,
        ),
        DownloadStatus::ExtractingMetadata => ("Extracting Metadata".to_owned(), theme.warning),
        DownloadStatus::Enriching => ("Enriching".to_owned(), theme.warning),
        DownloadStatus::Renaming => ("Renaming".to_owned(), theme.warning),
        DownloadStatus::Completed => ("Completed".to_owned(), theme.success),
        DownloadStatus::Failed(_) => ("Failed".to_owned(), theme.error),
    }
}

fn format_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if bytes >= MIB {
        format_decimal_bytes(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        format_decimal_bytes(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

fn format_decimal_bytes(bytes: u64, unit: u64, suffix: &str) -> String {
    let whole = bytes / unit;
    let decimal = bytes % unit * 10 / unit;
    format!("{whole}.{decimal} {suffix}")
}

fn render_discover(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let results_area = render_discover_inputs(frame, area, app, theme);
    render_discover_status(frame, results_area, app, theme);
}

fn render_discover_inputs(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) -> Rect {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(area);
    let show_filter = !app.discovery.results.is_empty();
    let inputs = Layout::horizontal(if show_filter {
        [Constraint::Percentage(55), Constraint::Percentage(45)]
    } else {
        [Constraint::Percentage(100), Constraint::Length(0)]
    })
    .split(rows[0]);
    let query = if app.discovery.query.is_empty() {
        "Search words | author: name | title: words | category: gr-qc"
    } else {
        &app.discovery.query
    };
    let search_style = if app.mode == AppMode::Search {
        Style::default().fg(theme.text).bg(theme.surface)
    } else if app.discovery.query.is_empty() {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.text)
    };
    frame.render_widget(
        Paragraph::new(format!("/ {query}"))
            .style(search_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if app.mode == AppMode::Search {
                        theme.accent
                    } else {
                        theme.border
                    })),
            ),
        inputs[0],
    );

    if show_filter {
        let filter = if app.discovery.filter.is_empty() {
            "Filter displayed results"
        } else {
            &app.discovery.filter
        };
        let filter_style = if app.mode == AppMode::DiscoverFilter {
            Style::default().fg(theme.text).bg(theme.surface)
        } else if app.discovery.filter.is_empty() {
            Style::default().fg(theme.muted)
        } else {
            Style::default().fg(theme.text)
        };
        frame.render_widget(
            Paragraph::new(format!("> {filter}"))
                .style(filter_style)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(
                            if app.mode == AppMode::DiscoverFilter {
                                theme.accent
                            } else {
                                theme.border
                            },
                        )),
                ),
            inputs[1],
        );
    }

    if app.mode == AppMode::Search {
        let cursor_offset = u16::try_from(
            app.discovery.query[..app.discovery.query_cursor]
                .chars()
                .count(),
        )
        .unwrap_or(0);
        frame.set_cursor_position((
            inputs[0].x.saturating_add(3).saturating_add(cursor_offset),
            inputs[0].y.saturating_add(1),
        ));
    }
    if show_filter && app.mode == AppMode::DiscoverFilter {
        let cursor_offset = u16::try_from(
            app.discovery.filter[..app.discovery.filter_cursor]
                .chars()
                .count(),
        )
        .unwrap_or(0);
        frame.set_cursor_position((
            inputs[1].x.saturating_add(3).saturating_add(cursor_offset),
            inputs[1].y.saturating_add(1),
        ));
    }

    rows[1]
}

fn render_discover_status(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    match &app.discovery.status {
        DiscoveryStatus::Idle => render_discover_empty(frame, area, theme),
        DiscoveryStatus::Loading if app.discovery.results.is_empty() => {
            frame.render_widget(
                Paragraph::new("Searching arXiv...")
                    .style(Style::default().fg(theme.accent))
                    .alignment(Alignment::Center),
                area,
            );
        }
        DiscoveryStatus::Error(_) if app.discovery.results.is_empty() => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("Search failed", Style::default().fg(theme.error)),
                    Line::raw(""),
                    Line::styled("Press r to retry", Style::default().fg(theme.text)),
                ])
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        DiscoveryStatus::Ready if app.discovery.results.is_empty() => {
            frame.render_widget(
                Paragraph::new("No papers found. Try a broader query.")
                    .style(Style::default().fg(theme.muted))
                    .alignment(Alignment::Center),
                area,
            );
        }
        DiscoveryStatus::Loading | DiscoveryStatus::Error(_) | DiscoveryStatus::Ready => {
            render_search_results(frame, area, app, theme);
        }
    }
}

fn render_discover_empty(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "Discover research on arXiv",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                "Press / and enter a query to begin.",
                Style::default().fg(theme.muted),
            ),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

fn render_search_results(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let progress = app
        .discovery
        .progress_message
        .as_deref()
        .unwrap_or_else(|| {
            if app.discovery.status == DiscoveryStatus::Loading {
                "Loading more results..."
            } else {
                ""
            }
        });
    let progress_prefix = if progress.is_empty() {
        String::new()
    } else {
        format!("{progress}  ")
    };
    let items = app.discovery.visible_page_results().map(|paper| {
        let local_status = app
            .downloaded_remote_paper(paper)
            .map(|local| local.reading_status.as_str());

        let has_local = app.downloaded_remote_paper(paper).is_some();

        let mut spans = vec![Span::styled(
            format!(
                "{}  |  {}  |  {}",
                format_local_datetime(&paper.published, "%Y-%m-%d"),
                compact_authors(paper),
                paper
                    .categories
                    .first()
                    .map_or("uncategorized", String::as_str)
            ),
            Style::default().fg(theme.muted),
        )];

        if let Some(status) = local_status {
            spans.push(Span::styled("  |  ", Style::default().fg(theme.border)));
            spans.push(Span::styled(
                status.to_string(),
                Style::default().fg(theme.muted),
            ));
        }

        if has_local {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                " (Downloaded) ",
                Style::default()
                    .fg(theme.success)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        workspace_list_item(vec![
            Line::styled(
                display_paper_title(&paper.title),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::from(spans),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " {}PAGE {} / {}  |  {} RESULTS  |  Ctrl+Left/Right: page ",
                    progress_prefix,
                    app.discovery.page + 1,
                    app.discovery.page_count(),
                    app.discovery.filtered_result_count(),
                ))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.discovery.selected))
        .with_offset(app.discovery.scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.discovery.scroll = state.offset();
    app.discovery.store_page_view();
}

fn compact_authors(paper: &RemotePaper) -> String {
    match paper.authors.as_slice() {
        [] => "Unknown authors".to_owned(),
        [author] => author.clone(),
        [first, second] => format!("{first}, {second}"),
        [first, ..] => format!("{first} et al."),
    }
}

fn render_paper_detail(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let paper = match app.page {
        Page::Dashboard => app.today_papers.get(app.today_selected),
        _ => app.discovery.selected_paper(),
    };
    let Some(paper) = paper else {
        return;
    };
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );
    let outer = area.inner(ratatui::layout::Margin {
        horizontal: 3,
        vertical: 1,
    });
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(outer);
    frame.render_widget(
        Paragraph::new("PAPER")
            .style(
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(theme.border)),
            ),
        rows[0],
    );
    let is_downloaded = app.downloaded_remote_paper(paper).is_some();
    frame.render_widget(
        Paragraph::new(paper_detail_lines(paper, theme, is_downloaded))
            .wrap(Wrap { trim: true })
            .scroll((app.paper_detail_scroll, 0)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(if is_downloaded {
            "j/k scroll  h back  Enter open PDF  d download  c cite  o browser"
        } else {
            "j/k scroll  h back  d download  c cite  o browser"
        })
        .style(Style::default().fg(theme.muted)),
        rows[2],
    );
    if let Some(toast) = &app.toast {
        frame.render_widget(
            Paragraph::new(toast.as_str())
                .alignment(Alignment::Right)
                .style(Style::default().fg(theme.success)),
            rows[2],
        );
    }
}

fn display_paper_title(title: &str) -> &str {
    title.strip_suffix(".pdf").unwrap_or(title)
}

fn paper_detail_lines<'a>(
    paper: &'a RemotePaper,
    theme: &Theme,
    is_downloaded: bool,
) -> Vec<Line<'a>> {
    let doi = paper.doi.as_deref().unwrap_or("Not available");
    let journal = paper.journal_ref.as_deref().unwrap_or("Not available");
    let pdf = paper.pdf_url.as_deref().unwrap_or("Not available");
    let mut title_spans = vec![Span::styled(
        display_paper_title(&paper.title),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    if is_downloaded {
        title_spans.push(Span::raw(" "));
        title_spans.push(Span::styled(
            " (Downloaded) ",
            Style::default()
                .fg(theme.success)
                .bg(theme.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    vec![
        Line::from(title_spans),
        Line::raw(""),
        Line::styled(paper.author_line(), Style::default().fg(theme.secondary)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Published  ", Style::default().fg(theme.muted)),
            Span::styled(
                format_local_datetime(&paper.published, "%B %d, %Y"),
                Style::default().fg(theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Updated    ", Style::default().fg(theme.muted)),
            Span::styled(
                format_local_datetime(&paper.updated, "%B %d, %Y"),
                Style::default().fg(theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Categories ", Style::default().fg(theme.muted)),
            Span::styled(
                paper.categories.join(", "),
                Style::default().fg(theme.warning),
            ),
        ]),
        Line::from(vec![
            Span::styled("DOI        ", Style::default().fg(theme.muted)),
            Span::styled(doi, Style::default().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("Journal    ", Style::default().fg(theme.muted)),
            Span::styled(journal, Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::styled(
            "ABSTRACT",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(&paper.abstract_text, Style::default().fg(theme.text)),
        Line::raw(""),
        Line::styled(
            "RESOURCES",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("arXiv  {}", paper.id),
            Style::default().fg(theme.accent),
        ),
        Line::styled(format!("PDF    {pdf}"), Style::default().fg(theme.accent)),
    ]
}

fn render_dashboard(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(6),
        Constraint::Length(6),
    ])
    .spacing(1)
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Today in Research",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );
    let cards = Layout::horizontal([Constraint::Ratio(1, 5); 5])
        .spacing(1)
        .split(rows[1]);
    let data = [
        ("LIBRARY", app.stats.papers, theme.accent),
        ("QUEUE", app.stats.queued, theme.warning),
        ("DOWNLOADED", app.stats.downloaded, theme.success),
        ("READ", app.dashboard.read, theme.secondary),
        (
            "STREAK",
            app.dashboard.reading.current_streak,
            theme.warning,
        ),
    ];
    for (area, (label, value, color)) in cards.iter().zip(data) {
        let card = Paragraph::new(vec![
            Line::styled(label, Style::default().fg(theme.muted)),
            Line::styled(
                value.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        )
        .alignment(Alignment::Center);
        frame.render_widget(card, *area);
    }
    render_today_research(frame, rows[2], app, theme);
    render_dashboard_details(frame, rows[3], app, theme);
}

fn render_today_research(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let preview_width = usize::from(area.width.saturating_sub(8)).max(24);
    let today = match &app.today_status {
        DiscoveryStatus::Loading => vec![workspace_list_item(vec![Line::raw(
            "Loading dashboard papers...",
        )])],
        DiscoveryStatus::Error(_) => vec![workspace_list_item(vec![Line::raw(
            "Dashboard papers are unavailable",
        )])],
        _ if app.today_papers.is_empty() => {
            vec![workspace_list_item(vec![Line::raw("No new papers loaded")])]
        }
        _ => app
            .today_papers
            .iter()
            .take(10)
            .map(|paper| {
                let abstract_preview = compact_text(&paper.abstract_text, preview_width);
                let downloaded = app.downloaded_remote_paper(paper);
                let local_status = downloaded.map(|local| local.reading_status.as_str());

                let mut meta_str = format!(
                    "{}  |  {}",
                    compact_authors(paper),
                    format_local_datetime(&paper.published, "%Y-%m-%d")
                );
                if let Some(status) = local_status {
                    meta_str.push_str("  |  ");
                    meta_str.push_str(status);
                }
                let mut meta_spans =
                    vec![Span::styled(meta_str, Style::default().fg(theme.accent))];
                if downloaded.is_some() {
                    meta_spans.push(Span::raw(" "));
                    meta_spans.push(Span::styled(
                        " (Downloaded) ",
                        Style::default()
                            .fg(theme.success)
                            .bg(theme.surface)
                            .add_modifier(Modifier::BOLD),
                    ));
                }

                workspace_list_item(vec![
                    Line::styled(
                        display_paper_title(&paper.title),
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Line::from(meta_spans),
                    Line::styled(abstract_preview, Style::default().fg(theme.muted)),
                ])
            })
            .collect(),
    };
    let list = List::new(today)
        .block(
            Block::default()
                .title(" DASHBOARD PAPERS - j/k SELECT  ENTER OPEN  o BROWSER ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.text))
        .highlight_symbol("> ");
    let selected =
        (app.content_focused && !app.today_papers.is_empty()).then_some(app.today_selected);
    let mut state = ListState::default()
        .with_selected(selected)
        .with_offset(app.today_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.today_scroll = state.offset();
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut shortened = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    shortened.push_str("...");
    shortened
}

fn render_dashboard_details(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let lower = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
        .spacing(1)
        .split(area);
    let activity_lines = app
        .dashboard
        .recent_activity
        .iter()
        .take(3)
        .map(|item| {
            Line::from(vec![
                Span::styled(
                    format!("{}  ", activity_kind(&item.kind)),
                    Style::default().fg(theme.accent),
                ),
                Span::styled(&item.label, Style::default().fg(theme.text)),
            ])
        })
        .collect::<Vec<_>>();
    let activity = Paragraph::new(activity_lines).block(
        Block::default()
            .title(" RECENT ACTIVITY ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );
    frame.render_widget(activity, lower[0]);
    frame.render_widget(
        Block::default()
            .title(" STORAGE ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        lower[1],
    );
    let storage = lower[1].inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!("PDFs      {}", format_bytes(app.dashboard.disk_usage)),
                Style::default().fg(theme.success),
            ),
            Line::styled(
                format!("Database  {}", format_bytes(app.dashboard.database_size)),
                Style::default().fg(theme.secondary),
            ),
            Line::styled(
                format!("Downloads {}", format_bytes(app.dashboard.downloads_size)),
                Style::default().fg(theme.accent),
            ),
            Line::styled(
                format!("Groups  {}", app.dashboard.collections),
                Style::default().fg(theme.muted),
            ),
        ]),
        storage,
    );
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let line = if app.page == Page::Projects
        && app.active_project.is_some()
        && app.project_pane == ProjectPane::FileTree
    {
        Line::from(vec![
            Span::styled(
                " n ",
                Style::default().fg(theme.background).bg(theme.accent),
            ),
            Span::styled(" new  ", Style::default().fg(theme.muted)),
            Span::styled(
                " enter/right ",
                Style::default().fg(theme.background).bg(theme.secondary),
            ),
            Span::styled(" open  ", Style::default().fg(theme.muted)),
            Span::styled(
                " esc ",
                Style::default().fg(theme.background).bg(theme.warning),
            ),
            Span::styled(" projects  ", Style::default().fg(theme.muted)),
            Span::styled(" x ", Style::default().fg(theme.background).bg(theme.error)),
            Span::styled(" delete", Style::default().fg(theme.muted)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " j/k ",
                Style::default().fg(theme.background).bg(theme.accent),
            ),
            Span::styled(" navigate  ", Style::default().fg(theme.muted)),
            Span::styled(
                " enter ",
                Style::default().fg(theme.background).bg(theme.secondary),
            ),
            Span::styled(" open  ", Style::default().fg(theme.muted)),
            Span::styled(
                " ? ",
                Style::default().fg(theme.background).bg(theme.warning),
            ),
            Span::styled(" help  ", Style::default().fg(theme.muted)),
            Span::styled(" q ", Style::default().fg(theme.background).bg(theme.error)),
            Span::styled(" quit", Style::default().fg(theme.muted)),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
    if let Some(toast) = &app.toast {
        frame.render_widget(
            Paragraph::new(toast.as_str())
                .alignment(Alignment::Right)
                .style(Style::default().fg(theme.success)),
            area,
        );
    }
}

fn render_terminal_command(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = centered(76, 20, frame.area());
    frame.render_widget(Clear, area);
    let working_directory = app.terminal_command_directory.as_ref().map_or_else(
        || "current application directory".to_owned(),
        |directory| directory.display().to_string(),
    );
    let output = if app.terminal_command_output.is_empty() {
        "Command output will appear here.".to_owned()
    } else {
        app.terminal_command_output.clone()
    };
    let mut output_lines = Vec::new();
    if !app.terminal_completions.is_empty() {
        output_lines.push(Line::styled(
            format!("Completions ({})", app.terminal_completions.len()),
            Style::default().fg(theme.muted),
        ));
        output_lines.extend(app.terminal_completions.iter().enumerate().map(
            |(index, candidate)| {
                let style = if app.terminal_completion_selected == Some(index) {
                    Style::default()
                        .bg(theme.surface)
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                Line::styled(
                    format!(
                        "{} {candidate}",
                        if app.terminal_completion_selected == Some(index) {
                            "›"
                        } else {
                            " "
                        }
                    ),
                    style,
                )
            },
        ));
        output_lines.push(Line::raw(""));
    }
    output_lines.extend(output.lines().map(|line| Line::raw(line.to_owned())));
    let sections = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(2),
        Constraint::Length(3),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(Style::default().bg(theme.surface))
            .wrap(Wrap { trim: false })
            .block(focus_block(" TERMINAL OUTPUT ", false, theme)),
        sections[0],
    );
    frame.render_widget(
        Paragraph::new(format!("cwd: {working_directory}"))
            .style(Style::default().fg(theme.muted).bg(theme.surface)),
        sections[1],
    );
    frame.render_widget(
        Paragraph::new(app.terminal_command.as_str())
            .style(Style::default().bg(theme.surface))
            .block(focus_block(
                " TERMINAL — ENTER RUN  ESC CLOSE ",
                true,
                theme,
            )),
        sections[2],
    );
    let cursor = app.terminal_command
        [..app.terminal_command_cursor.min(app.terminal_command.len())]
        .chars()
        .count();
    let cursor = saturating_u16(cursor);
    frame.set_cursor_position((sections[2].x.saturating_add(1 + cursor), sections[2].y + 1));
}

fn render_project_citation_search(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let area = project_citation_search_area(frame.area());
    frame.render_widget(Clear, area);

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(area);

    render_project_citation_query(frame, chunks[0], app, theme);
    render_project_citation_results(frame, chunks[1], app, theme);
}

fn render_project_citation_query(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let mode_hint = match app.project_citation_search_mode {
        ProjectCitationSearchMode::Local => {
            " LOCAL SEARCH  [ Tab: Online Search ]  [ Enter: Add ] "
        }
        ProjectCitationSearchMode::Online => {
            " ONLINE SEARCH  [ Tab: Local Search ]  [ Enter: Search / Add ] "
        }
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" ADD CITATION —{mode_hint}"));

    let query_text = &app.project_citation_query;
    let search_content = if query_text.is_empty() {
        Line::from(Span::styled(
            "Search papers by title or author...",
            Style::default().fg(theme.muted),
        ))
    } else {
        Line::from(Span::styled(
            query_text.as_str(),
            Style::default().fg(theme.text),
        ))
    };
    frame.render_widget(
        Paragraph::new(search_content)
            .style(Style::default().bg(theme.surface))
            .block(search_block),
        area,
    );

    let cursor_offset = app
        .project_citation_query
        .chars()
        .take(app.project_citation_cursor)
        .count();
    frame.set_cursor_position((
        area.x
            .saturating_add(1)
            .saturating_add(u16::try_from(cursor_offset).unwrap_or(0)),
        area.y.saturating_add(1),
    ));
}

fn render_project_citation_results(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let badge_width = CITATION_BADGE.len();
    let list_inner_width = usize::from(area.width).saturating_sub(5);
    let max_title_width = list_inner_width.saturating_sub(badge_width + 1).max(4);
    let max_authors_width = max_title_width + badge_width + 1;

    let items: Vec<ListItem> = app
        .project_citation_results
        .iter()
        .map(|paper| {
            // (Added) detection: compare by lowercased title against the set of
            // BibTeX titles already indexed from the project's .bib files.
            let is_added = !paper.title().is_empty()
                && app
                    .project_bib_titles
                    .contains(&paper.title().trim().to_lowercase());

            let raw_title = if paper.title().is_empty() {
                "[untitled]"
            } else {
                paper.title()
            };
            // Truncate title so badge always fits.
            let title_text = safe_truncate(raw_title, max_title_width);
            // Pad to a fixed width so the badge column is always at the same offset.
            let padded_title = format!("{title_text:<max_title_width$}");

            let badge_span = if is_added {
                Span::styled(
                    CITATION_BADGE,
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                // Reserve the column with blank space so unselected rows don't shift.
                Span::styled(" ".repeat(badge_width), Style::default())
            };

            let title_line = Line::from(vec![
                Span::styled(padded_title, Style::default().fg(theme.text)),
                badge_span,
            ]);

            let authors = paper.authors();
            let raw_authors = if authors.trim().is_empty() {
                "Unknown authors"
            } else {
                authors.trim()
            };
            let authors_text = safe_truncate(raw_authors, max_authors_width);
            let authors_line = Line::from(vec![Span::styled(
                authors_text,
                Style::default().fg(theme.muted),
            )]);

            workspace_list_item(vec![title_line, authors_line])
        })
        .collect();

    let results_title = app.project_citation_search_status.as_deref().map_or_else(
        || " RESULTS  ↑↓ navigate  Enter add  Esc close ".to_owned(),
        |status| format!(" RESULTS  {status}  ↑↓ navigate  Enter add  Esc close "),
    );
    let list = List::new(items)
        .block(
            Block::default()
                .title(results_title)
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .style(Style::default().bg(theme.surface))
                .border_style(Style::default().fg(theme.accent)),
        )
        .highlight_style(Style::default().bg(theme.border).fg(theme.accent))
        .highlight_symbol("> ");

    app.project_citation_selected = app
        .project_citation_selected
        .min(app.project_citation_results.len().saturating_sub(1));

    let mut state = ListState::default()
        .with_selected(if app.project_citation_results.is_empty() {
            None
        } else {
            Some(app.project_citation_selected)
        })
        .with_offset(app.project_citation_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.project_citation_scroll = state.offset();
}

fn project_citation_search_area(area: Rect) -> Rect {
    centered(90, 24, area)
}

fn render_palette(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let area = centered(45, 18, frame.area());
    frame.render_widget(Clear, area);

    // Split the area into Search Bar and Results List
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(area);

    // 1. Render the search input field
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(" BROWSE PAPR ");

    let query_text = &app.palette_query;
    frame.render_widget(
        Paragraph::new(if query_text.is_empty() {
            "Search workspaces, papers, groups, settings…"
        } else {
            query_text.as_str()
        })
        .style(Style::default().bg(theme.surface))
        .block(search_block),
        chunks[0],
    );

    // Render text cursor position
    let cursor_offset = app
        .palette_query
        .chars()
        .take(app.palette_query_cursor)
        .count();
    frame.set_cursor_position((
        chunks[0]
            .x
            .saturating_add(1)
            .saturating_add(u16::try_from(cursor_offset).unwrap_or(0)),
        chunks[0].y.saturating_add(1),
    ));

    // 2. Filter items and render list
    let filtered_pages = app.filtered_palette_items();
    let items = filtered_pages
        .iter()
        .map(|page| ListItem::new(Line::styled(page.title(), Style::default().fg(theme.text))));

    let list = List::new(items)
        .block(
            Block::default()
                .title(" DESTINATIONS ")
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .style(Style::default().bg(theme.surface))
                .border_style(Style::default().fg(theme.accent)),
        )
        .highlight_style(Style::default().bg(theme.border).fg(theme.accent))
        .highlight_symbol("> ");

    // Ensure selected index is valid for filtered items
    app.palette_selected = app
        .palette_selected
        .min(filtered_pages.len().saturating_sub(1));

    let mut state = ListState::default()
        .with_selected(if filtered_pages.is_empty() {
            None
        } else {
            Some(app.palette_selected)
        })
        .with_offset(app.palette_scroll);

    frame.render_stateful_widget(list, chunks[1], &mut state);
    app.palette_scroll = state.offset();
}

fn render_help(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let max_area = centered(160, frame.area().height.saturating_sub(4), frame.area());
    let sections = keyboard_reference();
    // A column needs enough room for a key and a useful description.  Reflow
    // whole sections at narrower sizes rather than squeezing the old fixed
    // three-column layout until the descriptions are unusable.
    let inner_width = max_area.width.saturating_sub(2);
    let column_count = match inner_width {
        0..=71 => 1,
        72..=104 => 2,
        _ => 3,
    };
    let groups = help_column_groups(sections, column_count, inner_width);
    let layout_area = Rect::new(0, 0, inner_width, 0);
    let columns = help_columns(layout_area, &groups);
    let rendered = groups
        .iter()
        .zip(columns.iter())
        .map(|(group, column)| format_help_column(group, column.width, theme))
        .collect::<Vec<_>>();
    let total_rows = rendered.iter().map(Vec::len).max().unwrap_or(0);
    let desired_height = u16::try_from(total_rows.saturating_add(2)).unwrap_or(u16::MAX);
    let height = desired_height.clamp(12, max_area.height);
    let area = centered(max_area.width, height, frame.area());
    frame.render_widget(Clear, area);
    let title = if area.width < 86 {
        " KEYBOARD HELP — ↑/↓ SCROLL  ?/ESC CLOSE "
    } else {
        " KEYBOARD REFERENCE — ↑/↓ SCROLL  PGUP/PGDN PAGE  HOME/END JUMP  ?/ESC/q CLOSE "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.surface))
        .border_style(Style::default().fg(theme.secondary));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let viewport_rows = inner.height as usize;
    let columns = help_columns(inner, &groups);
    app.help_scroll = app
        .help_scroll
        .min(total_rows.saturating_sub(viewport_rows));
    for (column, lines) in columns.iter().zip(rendered) {
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(app.help_scroll)
                    .take(viewport_rows)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().fg(theme.text)),
            *column,
        );
    }
}

#[derive(Clone)]
struct HelpSection {
    title: &'static str,
    scope: &'static [&'static str],
    entries: &'static [(&'static str, &'static str)],
}

fn section_group_height(group: &[HelpSection], col_width: u16) -> usize {
    if group.is_empty() {
        return 0;
    }
    let key_width = group
        .iter()
        .flat_map(|section| section.entries)
        .map(|(key, _)| key.len())
        .max()
        .unwrap_or(0);
    let prefix_width = key_width + 4;
    let desc_width = usize::from(col_width).saturating_sub(prefix_width).max(1);

    let mut height = 0;
    for (index, section) in group.iter().enumerate() {
        height += wrap_help_text(section.title, usize::from(col_width)).len();
        for scope in section.scope {
            height += wrap_help_text(scope, usize::from(col_width).saturating_sub(2)).len();
        }
        for (_key, desc) in section.entries {
            height += wrap_help_text(desc, desc_width).len();
        }
        if index + 1 < group.len() {
            height += 1;
        }
    }
    height
}

fn help_column_groups(
    sections: Vec<HelpSection>,
    columns: usize,
    width: u16,
) -> Vec<Vec<HelpSection>> {
    if columns <= 1 || sections.is_empty() {
        return vec![sections];
    }
    let columns = columns.min(sections.len());
    let column_count = saturating_u16(columns).max(1);
    let column_width =
        width.saturating_sub(column_count.saturating_sub(1).saturating_mul(2)) / column_count;
    let mut units = help_section_units(&sections);
    let global_unit = units
        .iter()
        .position(|unit| {
            unit.first()
                .is_some_and(|section| section.title == "GLOBAL & NAVIGATION")
        })
        .map(|index| units.remove(index));
    units.sort_by_key(|unit| std::cmp::Reverse(section_group_height(unit, column_width)));

    let mut groups = vec![Vec::new(); columns];
    let mut heights = vec![0usize; columns];
    if let Some(global_unit) = global_unit {
        heights[0] = section_group_height(&global_unit, column_width);
        groups[0] = global_unit;
    }
    for unit in units {
        let target = heights
            .iter()
            .enumerate()
            .min_by_key(|(_, height)| *height)
            .map_or(0, |(index, _)| index);
        if !groups[target].is_empty() {
            heights[target] += 1;
        }
        heights[target] += section_group_height(&unit, column_width);
        groups[target].extend(unit);
    }
    groups
}

fn help_section_units(sections: &[HelpSection]) -> Vec<Vec<HelpSection>> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < sections.len() {
        let next_title = sections.get(index + 1).map(|section| section.title);
        let keep_together = matches!(
            (sections[index].title, next_title),
            ("NOTES & PDF VIEWER", Some("INTERNAL PDF VIEWER"))
                | ("DOWNLOADS", Some("PROJECT LIST"))
        );
        if keep_together {
            units.push(vec![sections[index].clone(), sections[index + 1].clone()]);
            index += 2;
        } else {
            units.push(vec![sections[index].clone()]);
            index += 1;
        }
    }
    units
}

fn help_columns_widths(area_width: u16, groups: &[Vec<HelpSection>]) -> Vec<u16> {
    let count = groups.len();
    if count == 0 {
        return vec![];
    }
    if count == 1 {
        return vec![area_width];
    }

    let gap = 2u16;
    let count_u16 = saturating_u16(count).max(1);
    let total_gaps = count_u16.saturating_sub(1).saturating_mul(gap);
    let avail_width = area_width.saturating_sub(total_gaps);

    let mut prefix_widths = Vec::with_capacity(count);
    let mut ideal_desc_widths = Vec::with_capacity(count);
    let mut min_widths = Vec::with_capacity(count);
    let mut ideal_widths = Vec::with_capacity(count);

    for g in groups {
        let key_width = g
            .iter()
            .flat_map(|section| section.entries)
            .map(|(key, _)| key.len())
            .max()
            .unwrap_or(0);
        let prefix = saturating_u16(key_width.saturating_add(4));
        let max_desc = g
            .iter()
            .flat_map(|section| section.entries)
            .map(|(_, desc)| desc.len())
            .max()
            .map_or(16, saturating_u16);

        let ideal_desc = max_desc;
        let min_w = prefix + 10;
        let ideal_w = prefix + ideal_desc;

        prefix_widths.push(prefix);
        ideal_desc_widths.push(ideal_desc);
        min_widths.push(min_w);
        ideal_widths.push(ideal_w);
    }

    let min_sum: u16 = min_widths.iter().sum();
    let ideal_sum: u16 = ideal_widths.iter().sum();

    let mut widths = vec![0u16; count];

    if avail_width <= min_sum {
        let total_min = u32::from(min_sum.max(1));
        let mut allocated = 0u16;
        for i in 0..count {
            let w = u16::try_from((u32::from(min_widths[i]) * u32::from(avail_width)) / total_min)
                .unwrap_or(u16::MAX);
            widths[i] = w;
            allocated += w;
        }
        let mut rem = avail_width.saturating_sub(allocated);
        let mut i = 0;
        while rem > 0 && count > 0 {
            widths[i % count] += 1;
            rem -= 1;
            i += 1;
        }
    } else if avail_width <= ideal_sum {
        let extra = avail_width - min_sum;
        let needed_extra: Vec<u16> = ideal_widths
            .iter()
            .zip(min_widths.iter())
            .map(|(&ideal, &min)| ideal.saturating_sub(min))
            .collect();
        let total_needed_extra: u32 = needed_extra
            .iter()
            .map(|&x| u32::from(x))
            .sum::<u32>()
            .max(1);

        let mut allocated = 0u16;
        for i in 0..count {
            let add =
                u16::try_from((u32::from(extra) * u32::from(needed_extra[i])) / total_needed_extra)
                    .unwrap_or(u16::MAX);
            widths[i] = min_widths[i] + add;
            allocated += widths[i];
        }
        let mut rem = avail_width.saturating_sub(allocated);
        let mut i = 0;
        while rem > 0 && count > 0 {
            widths[i % count] += 1;
            rem -= 1;
            i += 1;
        }
    } else {
        let extra = avail_width - ideal_sum;
        let base_add = extra / count_u16;
        let mut rem = extra % count_u16;

        for i in 0..count {
            widths[i] = ideal_widths[i]
                + base_add
                + if rem > 0 {
                    rem -= 1;
                    1
                } else {
                    0
                };
        }
    }

    widths
}

fn help_columns(area: Rect, groups: &[Vec<HelpSection>]) -> Vec<Rect> {
    if groups.is_empty() {
        return vec![];
    }
    if groups.len() == 1 {
        return vec![area];
    }
    let widths = help_columns_widths(area.width, groups);
    let gap = 2u16;
    let mut rects = Vec::with_capacity(groups.len());
    let mut x = area.x;
    for &w in &widths {
        rects.push(Rect::new(x, area.y, w, area.height));
        x = x.saturating_add(w).saturating_add(gap);
    }
    rects
}

fn wrap_help_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let separator = usize::from(!line.is_empty());
        if !line.is_empty() && line.len() + separator + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        if word.len() <= width {
            line.push_str(word);
        } else {
            // No shortcut text currently needs this, but it keeps even an
            // unusually long future word inside the allocated column.
            for chunk in word.as_bytes().chunks(width) {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(String::from_utf8_lossy(chunk).into_owned());
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn format_help_column(group: &[HelpSection], width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let key_width = group
        .iter()
        .flat_map(|section| section.entries)
        .map(|(key, _)| key.len())
        .max()
        .unwrap_or(0);
    let prefix_width = key_width + 4; // two-space left indent and key/description gap
    let description_width = usize::from(width).saturating_sub(prefix_width).max(1);
    let mut lines = Vec::new();
    for (index, section) in group.iter().enumerate() {
        lines.extend(
            wrap_help_text(section.title, usize::from(width))
                .into_iter()
                .map(|line| {
                    Line::styled(
                        line,
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )
                }),
        );
        for scope in section.scope {
            lines.extend(
                wrap_help_text(scope, usize::from(width).saturating_sub(2))
                    .into_iter()
                    .map(|line| {
                        Line::styled(format!("  {line}"), Style::default().fg(theme.muted))
                    }),
            );
        }
        for (key, description) in section.entries {
            let description = wrap_help_text(description, description_width);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {key:<key_width$}  "),
                    Style::default().fg(theme.secondary),
                ),
                Span::styled(description[0].clone(), Style::default().fg(theme.text)),
            ]));
            for continuation in description.into_iter().skip(1) {
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(prefix_width)),
                    Span::styled(continuation, Style::default().fg(theme.text)),
                ]));
            }
        }
        if index + 1 < group.len() {
            lines.push(Line::raw(""));
        }
    }
    lines
}

/// The help overlay is deliberately organized by input behavior rather than
/// by destination. Each row has one command or one pair of equivalent keys,
/// making the reference readable without sacrificing accuracy.
fn keyboard_reference() -> Vec<HelpSection> {
    let mut sections = general_help_sections();
    sections.extend(project_help_sections());
    sections
}

fn general_help_sections() -> Vec<HelpSection> {
    vec![
        HelpSection {
            title: "GLOBAL & NAVIGATION",
            scope: &[],
            entries: &[
                ("?", "toggle reference"),
                ("/", "toggle arXiv search"),
                ("Ctrl+b", "toggle navigator"),
                ("Ctrl+t", "terminal palette"),
                ("Esc", "close local search"),
                ("Enter / → / l", "open selected item"),
                ("← / h", "return one level"),
                ("q", "quit outside text input"),
            ],
        },
        HelpSection {
            title: "PAPER ACTIONS",
            scope: &[
                "Paper rows: Library, Downloads, Bookmarks, Notes,",
                "Reading Queue, and paper rows in Groups or Authors.",
            ],
            entries: &[
                ("Enter / → / l", "open local PDF"),
                ("o", "open paper online"),
                ("B", "toggle bookmark"),
                (">", "toggle local search"),
                ("n", "open note"),
                ("g", "assign to group"),
                ("R", "rename PDF"),
                ("c", "copy citation"),
                ("x", "confirm then delete PDF"),
                ("a", "toggle reading queue"),
                ("u", "mark PDF unread"),
            ],
        },
        HelpSection {
            title: "NOTES & PDF VIEWER",
            scope: &[],
            entries: &[
                ("Tab", "toggle edit / preview"),
                ("Esc", "save and leave note"),
            ],
        },
        HelpSection {
            title: "INTERNAL PDF VIEWER",
            scope: &[],
            entries: &[("Esc / q", "close internal viewer")],
        },
        HelpSection {
            title: "CREDITS",
            scope: &[],
            entries: &[("Enter", "open selected link")],
        },
        HelpSection {
            title: "DISCOVER & DASHBOARD",
            scope: &[],
            entries: &[
                ("/", "toggle arXiv search"),
                ("d", "download paper"),
                ("Enter", "open downloaded PDF"),
            ],
        },
        HelpSection {
            title: "DISCOVER",
            scope: &[],
            entries: &[
                ("/", "toggle arXiv search"),
                ("Ctrl+←", "previous cached page"),
                ("Ctrl+→", "next cached page"),
                ("r", "retry loading failure"),
            ],
        },
        HelpSection {
            title: "GROUPS",
            scope: &[],
            entries: &[("g", "create a group")],
        },
        HelpSection {
            title: "QUEUE",
            scope: &[],
            entries: &[
                ("Shift+↑", "move queued paper up"),
                ("Shift+↓", "move queued paper down"),
            ],
        },
        HelpSection {
            title: "DOWNLOADS",
            scope: &[],
            entries: &[("r", "retry failed download")],
        },
    ]
}

fn project_help_sections() -> Vec<HelpSection> {
    vec![
        HelpSection {
            title: "PROJECT LIST",
            scope: &[],
            entries: &[
                ("n", "create named project"),
                ("r", "refresh project list"),
                ("R", "rename selected project"),
                ("x", "delete selected project"),
            ],
        },
        HelpSection {
            title: "PROJECT FILE TREE",
            scope: &["Shows folders, source files, and supported image files."],
            entries: &[
                ("Enter / →", "open file or enter folder"),
                ("←", "parent folder; exit at project root"),
                ("n", "create file; add / for folder"),
                ("R", "rename selected file or folder"),
                ("x", "confirm then delete file or folder"),
                ("Ctrl+f", "search and add citation"),
                ("Esc", "return to project list at root"),
            ],
        },
        HelpSection {
            title: "PROJECT PANES & PREVIEW",
            scope: &[],
            entries: &[
                ("Alt+1", "focus file tree"),
                ("Alt+2", "focus editor"),
                ("Alt+3", "focus PDF preview"),
                ("Alt+4", "focus compiler output"),
            ],
        },
        HelpSection {
            title: "PROJECT EDITOR (NORMAL MODE)",
            scope: &[],
            entries: &[
                ("i", "enter Insert mode"),
                ("w / b", "next / previous word"),
                ("0/$, gg/G", "line/file bounds"),
                ("dd", "delete current line"),
                ("x / Delete", "delete character"),
                ("V, j/k, y/d", "select lines, move, yank/delete"),
                ("u / Ctrl+r", "undo/redo; Ctrl+Bksp/Delete word"),
                ("PgUp/PgDn", "page move; wheel scrolls"),
                ("Esc", "focus file tree"),
                ("Ctrl+s", "save current source"),
                ("Ctrl+Shift+v", "paste exactly into .tex/.bib and save"),
            ],
        },
    ]
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(
        Paragraph::new("papr needs a terminal at least 58 x 18")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.warning)),
        Rect::new(area.x, area.y + area.height / 2, area.width, 1),
    );
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn render_credits(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    render_credits_about(frame, chunks[0], theme);
    render_credits_links(frame, chunks[1], app, theme);
}

fn render_credits_about(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut left_lines = credits_overview_lines(theme);
    left_lines.extend(credits_technology_lines(theme));

    let left_para = Paragraph::new(left_lines)
        .block(
            Block::default()
                .title(" ABOUT PAPR ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .style(Style::default().bg(theme.surface)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(left_para, area);
}

fn credits_overview_lines(theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::styled(
            "Papr - Academic TUI Workspace",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            "A keyboard-first terminal workspace for researchers.",
            Style::default().fg(theme.text),
        ),
        Line::raw(""),
        Line::styled(
            "PROJECT DETAILS",
            Style::default()
                .fg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            Span::styled("Version:        ", Style::default().fg(theme.muted)),
            Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("License:        ", Style::default().fg(theme.muted)),
            Span::styled("MIT", Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("Authors:        ", Style::default().fg(theme.muted)),
            Span::styled("Saqlain Afroz & Tanveer", Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::styled(
            "RESEARCH DATA PROVIDERS & APIS",
            Style::default()
                .fg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            "• arXiv API (Preprint search and metadata retrieval)",
            Style::default().fg(theme.text),
        ),
        Line::styled(
            "• Crossref API (DOI resolution and BibTeX citation metadata)",
            Style::default().fg(theme.text),
        ),
        Line::styled(
            "• OpenAlex API (Open catalog of scholarly work and journal metadata)",
            Style::default().fg(theme.text),
        ),
        Line::raw(""),
    ]
}

fn credits_technology_lines(theme: &Theme) -> Vec<Line<'static>> {
    let heading_style = Style::default()
        .fg(theme.secondary)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.text);
    vec![
        Line::styled("CORE TECHNOLOGIES USED", heading_style),
        Line::styled("• Rust (Robust system programming language)", text_style),
        Line::styled(
            "• Ratatui & Crossterm (Terminal rendering and TUI library)",
            text_style,
        ),
        Line::styled(
            "• SQLite (Local metadata storage and persistence)",
            text_style,
        ),
        Line::styled(
            "• Tokio (Asynchronous task pool and download scheduler)",
            text_style,
        ),
        Line::styled(
            "• Reqwest (HTTP client for querying search APIs)",
            text_style,
        ),
        Line::styled(
            "• Typst (Markup-based document typesetting and embedded PDF compiler)",
            text_style,
        ),
        Line::styled(
            "• ratatui-image & Poppler (Terminal PDF rendering protocol)",
            text_style,
        ),
        Line::styled(
            "• pulldown-cmark & syntect (Markdown note parsing and syntax highlighting)",
            text_style,
        ),
        Line::styled(
            "• arboard (Cross-platform system clipboard integration)",
            text_style,
        ),
        Line::styled(
            "• iana-time-zone & Chrono (Cross-platform timezone detection and formatting)",
            text_style,
        ),
        Line::raw(""),
        Line::styled("SPECIAL THANKS", heading_style),
        Line::styled("Dedicated to the open-source community,", text_style),
        Line::styled(
            "and academic authors worldwide who share their work freely.",
            text_style,
        ),
        Line::styled("To our parents who always supported us.", text_style),
    ]
}

fn render_credits_links(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let right_chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).split(area);

    let credits_items = app.credits_items();
    let list_items: Vec<ListItem> = credits_items
        .iter()
        .map(|item| {
            workspace_list_item(vec![
                Line::styled(&item.label, Style::default().fg(theme.text)),
                Line::styled(&item.url, Style::default().fg(theme.muted)),
            ])
        })
        .collect();

    let list = List::new(list_items)
        .block(
            Block::default()
                .title(" INTERACTIVE LINKS & DEPENDENCIES ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .style(Style::default().bg(theme.surface)),
        )
        .highlight_style(Style::default().bg(theme.border).fg(theme.accent))
        .highlight_symbol("> ");

    // Safety check selection boundaries
    app.credits_selected = app
        .credits_selected
        .min(credits_items.len().saturating_sub(1));

    let mut state = ListState::default()
        .with_selected(workspace_highlight_selection(app, app.credits_selected))
        .with_offset(app.credits_scroll);

    frame.render_stateful_widget(list, right_chunks[0], &mut state);
    app.credits_scroll = state.offset();

    let instructions = Line::styled(
        " ▲/▼: Select  |  Enter: Open link in default browser",
        Style::default().fg(theme.muted),
    );
    frame.render_widget(Paragraph::new(instructions), right_chunks[1]);
}

fn get_paper_collection_name(app: &App, paper_id: i64) -> Option<String> {
    for (collection_id, paper_ids) in &app.collection_papers_map {
        if paper_ids.contains(&paper_id)
            && let Some(collection) = app.collections.iter().find(|c| c.id == *collection_id)
        {
            return Some(collection.name.clone());
        }
    }
    None
}

fn safe_truncate(s: &str, max_width: usize) -> String {
    if s.chars().count() <= max_width {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_width.saturating_sub(3)).collect();
        truncated.push_str("...");
        truncated
    }
}

fn wrap_text_to_spans(
    label_prefix: &str,
    text: &str,
    avail_width: usize,
    label_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let avail = avail_width.saturating_sub(2).max(15);
    let label_len = label_prefix.chars().count();
    let indent = " ".repeat(label_len);
    let first_max = avail.saturating_sub(label_len).max(5);
    let cont_max = avail.saturating_sub(indent.len()).max(5);

    let mut result_lines = Vec::new();
    let mut is_first_line_of_section = true;

    for raw_line in text.lines() {
        let words: Vec<&str> = raw_line.split_whitespace().collect();
        if words.is_empty() {
            let prefix = if is_first_line_of_section {
                label_prefix.to_string()
            } else {
                indent.clone()
            };
            result_lines.push(Line::from(vec![Span::styled(prefix, label_style)]));
            is_first_line_of_section = false;
            continue;
        }

        let mut current_prefix = if is_first_line_of_section {
            label_prefix.to_string()
        } else {
            indent.clone()
        };
        let mut current_max = if is_first_line_of_section {
            first_max
        } else {
            cont_max
        };
        let mut current_words = Vec::new();
        let mut current_len = 0;

        for word in words {
            let word_len = word.chars().count();
            if current_words.is_empty() {
                if word_len > current_max {
                    let chars: Vec<char> = word.chars().collect();
                    let mut start = 0;
                    while start < chars.len() {
                        let end = (start + current_max).min(chars.len());
                        let chunk: String = chars[start..end].iter().collect();
                        result_lines.push(Line::from(vec![
                            Span::styled(current_prefix.clone(), label_style),
                            Span::styled(chunk, text_style),
                        ]));
                        start = end;
                        current_prefix.clone_from(&indent);
                        current_max = cont_max;
                        is_first_line_of_section = false;
                    }
                } else {
                    current_words.push(word);
                    current_len = word_len;
                }
            } else if current_len + 1 + word_len <= current_max {
                current_words.push(word);
                current_len += 1 + word_len;
            } else {
                let line_str = current_words.join(" ");
                result_lines.push(Line::from(vec![
                    Span::styled(current_prefix.clone(), label_style),
                    Span::styled(line_str, text_style),
                ]));
                current_prefix.clone_from(&indent);
                current_max = cont_max;
                is_first_line_of_section = false;
                current_words.clear();

                if word_len > current_max {
                    let chars: Vec<char> = word.chars().collect();
                    let mut start = 0;
                    while start < chars.len() {
                        let end = (start + current_max).min(chars.len());
                        let chunk: String = chars[start..end].iter().collect();
                        result_lines.push(Line::from(vec![
                            Span::styled(current_prefix.clone(), label_style),
                            Span::styled(chunk, text_style),
                        ]));
                        start = end;
                    }
                    current_len = 0;
                } else {
                    current_words.push(word);
                    current_len = word_len;
                }
            }
        }

        if !current_words.is_empty() {
            let line_str = current_words.join(" ");
            result_lines.push(Line::from(vec![
                Span::styled(current_prefix, label_style),
                Span::styled(line_str, text_style),
            ]));
            is_first_line_of_section = false;
        }
    }

    ensure_wrapped_line(&mut result_lines, label_prefix, label_style);

    result_lines
}

fn ensure_wrapped_line(lines: &mut Vec<Line<'static>>, label_prefix: &str, label_style: Style) {
    if lines.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            label_prefix.to_string(),
            label_style,
        )]));
    }
}

#[derive(Clone, Copy)]
struct PaperLineContext<'a> {
    paper_id: Option<i64>,
    title: &'a str,
    authors: &'a str,
    reading_status: &'a str,
    file_size: Option<u64>,
    availability: Option<&'a str>,
    bookmark_year: Option<&'a str>,
    bookmark_journal: Option<&'a str>,
    bookmark_doi: Option<&'a str>,
    bookmark_page: Option<u64>,
    download_label: Option<(&'a str, ratatui::style::Color)>,
    available_width: usize,
}

fn build_paper_lines<'a>(app: &App, theme: &Theme, context: PaperLineContext<'a>) -> Vec<Line<'a>> {
    let PaperLineContext {
        title,
        authors,
        available_width,
        ..
    } = context;
    let stats_spans = paper_stats_spans(app, theme, context);

    let stats_len: usize = stats_spans.iter().map(ratatui::prelude::Span::width).sum();
    let authors_str = if authors.is_empty() {
        "Unknown authors"
    } else {
        authors
    };
    let separator_len = if stats_spans.is_empty() { 0 } else { 5 };
    let max_authors_width = available_width
        .saturating_sub(stats_len)
        .saturating_sub(separator_len);

    if stats_spans.is_empty()
        || authors_str.chars().count() <= max_authors_width
        || max_authors_width >= 20
    {
        let display_authors = if authors_str.chars().count() <= max_authors_width {
            authors_str.to_string()
        } else {
            safe_truncate(authors_str, max_authors_width)
        };

        let mut line_spans = vec![Span::styled(
            display_authors,
            Style::default().fg(theme.muted),
        )];
        if !stats_spans.is_empty() {
            line_spans.push(Span::styled("  |  ", Style::default().fg(theme.border)));
            line_spans.extend(stats_spans);
        }
        vec![
            Line::styled(
                title.to_string(),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::from(line_spans),
        ]
    } else {
        let mut lines = vec![
            Line::styled(
                title.to_string(),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                safe_truncate(authors_str, available_width),
                Style::default().fg(theme.muted),
            ),
        ];
        if !stats_spans.is_empty() {
            lines.push(Line::from(stats_spans));
        }
        lines
    }
}

fn paper_stats_spans<'a>(app: &App, theme: &Theme, context: PaperLineContext<'a>) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    if !context.reading_status.is_empty() {
        push_paper_stat(
            &mut spans,
            context.reading_status.to_owned(),
            theme.muted,
            theme,
        );
    }
    if let Some(year) = context.bookmark_year {
        push_paper_stat(&mut spans, year.to_owned(), theme.muted, theme);
    }
    if let Some(journal) = context.bookmark_journal {
        push_paper_stat(&mut spans, journal.to_owned(), theme.muted, theme);
    } else if let Some(doi) = context.bookmark_doi {
        push_paper_stat(&mut spans, format!("DOI {doi}"), theme.muted, theme);
    }
    if let Some(page) = context.bookmark_page {
        push_paper_stat(&mut spans, format!("page {page}"), theme.muted, theme);
    }
    if let Some(name) = context
        .paper_id
        .and_then(|id| get_paper_collection_name(app, id))
    {
        push_paper_separator(&mut spans, theme);
        spans.push(Span::styled("(", Style::default().fg(theme.muted)));
        spans.push(Span::styled(
            name,
            Style::default()
                .bg(theme.surface)
                .fg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(")", Style::default().fg(theme.muted)));
    }
    if let Some(size) = context.file_size {
        push_paper_stat(&mut spans, format_bytes(size), theme.muted, theme);
    } else if context.bookmark_year.is_none()
        && context.download_label.is_none()
        && context.paper_id.is_some()
    {
        push_paper_stat(&mut spans, "metadata only".to_owned(), theme.muted, theme);
    }
    if let Some(availability) = context.availability {
        push_paper_stat(&mut spans, availability.to_owned(), theme.muted, theme);
    }
    if let Some((label, color)) = context.download_label {
        push_paper_stat(&mut spans, label.to_owned(), color, theme);
    }
    spans
}

fn push_paper_separator(spans: &mut Vec<Span<'_>>, theme: &Theme) {
    if !spans.is_empty() {
        spans.push(Span::styled("  |  ", Style::default().fg(theme.border)));
    }
}

fn push_paper_stat(
    spans: &mut Vec<Span<'_>>,
    value: String,
    color: ratatui::style::Color,
    theme: &Theme,
) {
    push_paper_separator(spans, theme);
    spans.push(Span::styled(value, Style::default().fg(color)));
}

fn render_summary_modal(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let centered_rect = |percent_x, percent_y, r: ratatui::layout::Rect| {
        let popup_layout = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
                ratatui::layout::Constraint::Percentage(percent_y),
                ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
                ratatui::layout::Constraint::Percentage(percent_x),
                ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    };

    let modal_area = centered_rect(80, 80, frame.area());
    frame.render_widget(ratatui::widgets::Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" AI Summary ")
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background).fg(theme.text));

    let content_area = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    match &app.summary_state {
        Some(crate::state::SummaryState::Generating(model)) => {
            let text = format!("Generating summary using {}...\nThis may take a minute or two.", model);
            let p = Paragraph::new(text)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true });
            let centered = centered_rect(60, 20, content_area);
            frame.render_widget(p, centered);
        }
        Some(crate::state::SummaryState::Error(err)) => {
            let text = format!("Error generating summary:\n\n{}", err);
            let p = Paragraph::new(text)
                .style(Style::default().fg(ratatui::style::Color::Red))
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true });
            let centered = centered_rect(80, 50, content_area);
            frame.render_widget(p, centered);
        }
        Some(crate::state::SummaryState::Ready(summary)) => {
            let preview = markdown_preview(summary.as_str(), theme);
            let p = Paragraph::new(preview.lines)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .scroll((app.summary_scroll as u16, 0));
            frame.render_widget(p, content_area);
            frame.render_widget(
                MarkdownHyperlinks::new(preview.hyperlinks, app.summary_scroll as u16),
                content_area,
            );
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::state::*;
    use crate::theme::*;
    use chrono::{TimeZone, Utc};
    use papr_core::models::AuthorSummary;
    use papr_core::{ActivityItem, BookmarkSummary, CollectionSummary, LibraryPaper, RemotePaper};
    use ratatui::{
        Terminal, backend::TestBackend, buffer::Buffer, layout::Rect, style::Style, widgets::Widget,
    };

    use super::{
        MarkdownHyperlinks, MarkdownLink, format_help_column, help_column_groups, help_columns,
        keyboard_reference, markdown_preview, render, workspace_highlight_selection,
    };

    #[test]
    fn keyboard_reference_reflows_without_exceeding_each_column()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::load("nord")?;
        for (width, count) in [(54, 1), (76, 2), (130, 3)] {
            let groups = help_column_groups(keyboard_reference(), count, width);
            let columns = help_columns(Rect::new(0, 0, width, 30), &groups);
            for (group, column) in groups.iter().zip(columns) {
                assert!(
                    format_help_column(group, column.width, &theme)
                        .iter()
                        .all(|line| line.width() <= usize::from(column.width))
                );
            }
        }
        Ok(())
    }

    #[test]
    fn keyboard_reference_balances_column_heights_and_widths()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::load("nord")?;

        // Check 3-column layout on wide screen
        let groups_3col = help_column_groups(keyboard_reference(), 3, 130);
        assert_eq!(groups_3col.len(), 3);
        let cols_3col = help_columns(Rect::new(0, 0, 130, 30), &groups_3col);
        let heights_3col: Vec<usize> = groups_3col
            .iter()
            .zip(&cols_3col)
            .map(|(g, col)| format_help_column(g, col.width, &theme).len())
            .collect();
        let max_h3 = heights_3col.iter().max().copied().ok_or("missing column")?;
        let min_h3 = heights_3col.iter().min().copied().ok_or("missing column")?;
        // Heights should be well balanced: max height should be <= 42 lines and diff <= 10 lines
        assert!(
            max_h3 <= 42,
            "Max height in 3-col layout should be <= 42, got {max_h3}"
        );
        assert!(
            max_h3 - min_h3 <= 10,
            "Height diff in 3-col layout should be <= 10, got {}",
            max_h3 - min_h3
        );

        // Check 2-column layout on medium screen
        let groups_2col = help_column_groups(keyboard_reference(), 2, 86);
        assert_eq!(groups_2col.len(), 2);
        let cols_2col = help_columns(Rect::new(0, 0, 86, 30), &groups_2col);
        let heights_2col: Vec<usize> = groups_2col
            .iter()
            .zip(&cols_2col)
            .map(|(g, col)| format_help_column(g, col.width, &theme).len())
            .collect();
        let max_h2 = heights_2col.iter().max().copied().ok_or("missing column")?;
        let min_h2 = heights_2col.iter().min().copied().ok_or("missing column")?;
        assert!(
            max_h2 - min_h2 <= 10,
            "Height diff in 2-col layout should be <= 10, got {}",
            max_h2 - min_h2
        );
        Ok(())
    }

    #[test]
    fn workspace_selection_highlight_is_only_rendered_while_content_has_focus() {
        let mut app = App {
            content_focused: false,
            ..App::default()
        };

        assert_eq!(workspace_highlight_selection(&app, 4), None);

        app.content_focused = true;
        assert_eq!(workspace_highlight_selection(&app, 4), Some(4));
    }

    #[test]
    fn inactive_workspace_hides_the_row_highlight_without_changing_selection_or_scroll()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let theme = Theme::load("nord")?;
        let mut app = App {
            page: Page::Library,
            content_focused: false,
            library: LibraryState {
                selected: 0,
                scroll: 0,
                papers: vec![LibraryPaper {
                    id: 1,
                    title: "Focus Aware Paper".into(),
                    authors: "Test Author".into(),
                    doi: None,
                    arxiv_id: None,
                    pdf_path: None,
                    file_size: None,
                    reading_status: "unread".into(),
                    is_favorite: false,
                }],
                ..LibraryState::default()
            },
            ..App::default()
        };

        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let buffer = terminal.backend().buffer();
        let index = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == "F")
            .ok_or("paper title should be rendered")?;
        let x = u16::try_from(index % usize::from(buffer.area.width)).unwrap_or(0);
        let y = u16::try_from(index / usize::from(buffer.area.width)).unwrap_or(0);
        assert_ne!(buffer[(x - 2, y)].symbol(), ">");
        assert_ne!(buffer[(x, y)].style().bg, Some(theme.surface));
        assert_eq!(app.library.selected, 0);
        assert_eq!(app.library.scroll, 0);

        app.content_focused = true;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let buffer = terminal.backend().buffer();
        let index = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == "F")
            .ok_or("paper title should be rendered")?;
        let x = u16::try_from(index % usize::from(buffer.area.width)).unwrap_or(0);
        let y = u16::try_from(index / usize::from(buffer.area.width)).unwrap_or(0);
        assert_eq!(buffer[(x - 2, y)].symbol(), ">");
        assert_eq!(buffer[(x, y)].style().bg, Some(theme.surface));
        assert_eq!(app.library.selected, 0);
        assert_eq!(app.library.scroll, 0);
        Ok(())
    }

    #[test]
    fn markdown_hyperlinks_keep_link_text_intact() {
        let area = Rect::new(0, 0, 32, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "A link: Papr on GitHub", Style::default());
        MarkdownHyperlinks::new(
            vec![MarkdownLink {
                line: 0,
                column: 8,
                width: 14,
                destination: "https://github.com/AfrozSaqlain/Papr".into(),
            }],
            0,
        )
        .render(area, &mut buffer);
        assert!(buffer[(8, 0)].symbol().contains("\x07Pa\x1b]8;;\x07"));
        assert!(buffer[(10, 0)].symbol().contains("\x07pr\x1b]8;;\x07"));
    }

    #[test]
    fn markdown_preview_supports_gfm_extensions() -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::load("nord")?;
        let markdown = r"# Research

| Method | Score |
| --- | ---: |
| Baseline | 0.8 |

- [x] complete
  - nested ~~old~~ item

> quoted [source](https://example.com)

Inline $E = mc^2$ and block:

$$
\int_0^1 x dx
$$

```rust
let answer = 42;
```

Image: ![plot](plot.png)[^1]

[^1]: A figure.
";
        let preview = markdown_preview(markdown, &theme);
        assert!(preview.lines.iter().any(|line| line.spans.is_empty()));
        assert!(
            preview
                .hyperlinks
                .iter()
                .any(|link| link.destination == "https://example.com")
        );
        let rendered = preview
            .lines
            .into_iter()
            .flat_map(|line| line.spans.into_iter())
            .map(|span| span.content.into_owned())
            .collect::<String>();

        for expected in [
            "Method",
            "Baseline",
            "[x]",
            "nested",
            "old",
            "│",
            "source",
            "E = mc²",
            "∫₀¹ x dx",
            "Code (rust)",
            "let answer = 42;",
            "🖼",
            "plot.png",
            "[^1]",
            "A figure.",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in {rendered:?}"
            );
        }
        assert!(!rendered.contains("https://example.com"));
        Ok(())
    }

    #[test]
    fn latex_math_renderer_handles_nested_constructs_and_malformed_source() {
        assert_eq!(
            super::format_math(r"\frac{\Sigma_{i=1}^{n}}{\sqrt{1+\alpha^2}}"),
            "(Σᵢ₌₁ⁿ)⁄(√(1+α²))"
        );
        assert_eq!(super::format_math(r"\Sigma_{x = 1}^{\infty}"), "Σₓ₌₁⁽∞⁾");
        assert_eq!(super::format_math(r"\int_0^\infty"), "∫₀⁽∞⁾");
        assert_eq!(
            super::format_math(
                r"\left\langle\begin{bmatrix}a_i & \frac{1}{2}\\\mathbf{v} & \operatorname{rank}(A)\end{bmatrix}\right\rangle"
            ),
            "⟨[aᵢ  (1)⁄(2) ; v  rank(A)]⟩"
        );
        assert_eq!(
            super::format_math(r"\text{cost: \$5} + \mathbb{R}"),
            "cost: $5 + ℝ"
        );
        assert_eq!(super::format_math(r"\frac{a}{b"), r"\frac{a}{b");
    }

    #[test]
    fn markdown_preview_preserves_consecutive_blank_lines() -> Result<(), Box<dyn std::error::Error>>
    {
        let theme = Theme::load("nord")?;
        let preview = markdown_preview("first\n\n\nsecond\n\n\n\nthird", &theme);
        let lines = preview
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, ["first", "", "", "second", "", "", "", "third"]);
        Ok(())
    }

    #[test]
    fn dashboard_renders_at_minimum_size() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App::default();
        let theme = Theme::load("catppuccin")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Today in Research"));
        Ok(())
    }

    #[test]
    fn dashboard_feed_renders_title_authors_and_abstract() -> Result<(), Box<dyn std::error::Error>>
    {
        let backend = TestBackend::new(110, 32);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App {
            today_papers: vec![sample_paper()?],
            today_status: DiscoveryStatus::Ready,
            ..App::default()
        };
        let theme = Theme::load("nord")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Terminal Research Systems"));
        assert!(rendered.contains("Ada Lovelace, Alan Turing"));
        assert!(rendered.contains("A testable abstract"));
        Ok(())
    }

    #[test]
    fn discovery_results_and_detail_render() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App {
            page: Page::Discover,
            ..App::default()
        };
        app.discovery.status = DiscoveryStatus::Ready;
        app.discovery.results.push(sample_paper()?);
        let theme = Theme::load("nord")?;

        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        assert!(rendered_text(&terminal).contains("Terminal Research Systems"));

        app.mode = AppMode::PaperDetail;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let detail = rendered_text(&terminal);
        assert!(detail.contains("ABSTRACT"));
        assert!(detail.contains("10.1000/papr"));
        Ok(())
    }

    #[test]
    fn dashboard_paper_detail_does_not_reuse_discover_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let published = Utc
            .with_ymd_and_hms(2026, 1, 3, 0, 0, 0)
            .single()
            .ok_or("invalid test date")?;
        let dashboard_paper = RemotePaper {
            id: "https://arxiv.org/abs/dashboard".into(),
            title: "Dashboard Detail Paper".into(),
            authors: vec!["Ada Lovelace".into()],
            abstract_text: "Dashboard detail abstract.".into(),
            published,
            updated: published,
            categories: vec!["cs.HC".into()],
            pdf_url: Some("https://arxiv.org/pdf/dashboard".into()),
            doi: Some("10.1000/dashboard".into()),
            journal_ref: Some("Dashboard Journal".into()),
        };
        let discover_paper = RemotePaper {
            id: "https://arxiv.org/abs/discover".into(),
            title: "Discover Result Paper".into(),
            authors: vec!["Alan Turing".into()],
            abstract_text: "Discover result abstract.".into(),
            published,
            updated: published,
            categories: vec!["cs.DL".into()],
            pdf_url: Some("https://arxiv.org/pdf/discover".into()),
            doi: Some("10.1000/discover".into()),
            journal_ref: Some("Discover Journal".into()),
        };
        let mut app = App {
            page: Page::Dashboard,
            mode: AppMode::PaperDetail,
            today_papers: vec![dashboard_paper],
            today_selected: 0,
            discovery: DiscoveryState {
                results: vec![discover_paper],
                selected: 0,
                status: DiscoveryStatus::Ready,
                ..DiscoveryState::default()
            },
            ..App::default()
        };
        let theme = Theme::load("nord")?;

        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Dashboard Detail Paper"));
        assert!(!rendered.contains("Discover Result Paper"));
        Ok(())
    }

    #[test]
    fn library_and_downloads_render() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App {
            page: Page::Library,
            ..App::default()
        };
        app.library.papers.push(LibraryPaper {
            id: 1,
            title: "A Local Research Paper".into(),
            authors: "Researcher One".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("paper.pdf".into()),
            file_size: Some(2048),
            reading_status: "unread".into(),
            is_favorite: false,
        });
        let theme = Theme::load("gruvbox")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        assert!(rendered_text(&terminal).contains("paper"));
        assert!(!rendered_text(&terminal).contains("paper.pdf"));

        app.page = Page::Downloads;
        app.downloads.push(DownloadTask {
            id: "arxiv:test".into(),
            title: "A Streaming Download".into(),
            downloaded: 1024,
            total: Some(2048),
            paper_id: None,
            pdf_path: None,
            status: DownloadStatus::Running,
            remote_paper: None,
            failed_at: None,
        });
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let text = rendered_text(&terminal);
        assert!(text.contains("A Streaming Download"));
        assert!(text.contains("1.0 KiB / 2.0 KiB"));
        Ok(())
    }

    #[test]
    fn history_and_statistics_render() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let timestamp = Utc
            .with_ymd_and_hms(2026, 7, 13, 10, 0, 0)
            .single()
            .ok_or("invalid test date")?;
        let mut app = App {
            page: Page::History,
            ..App::default()
        };
        app.dashboard.recent_activity.push(ActivityItem {
            kind: "pdf_opened".into(),
            label: "A Recorded Paper".into(),
            occurred_at: timestamp,
        });
        app.dashboard.reading.current_streak = 4;
        app.dashboard.reading.monthly_reading = 12;
        app.dashboard.reading.most_active_day = Some("Monday".into());
        let theme = Theme::load("tokyo-night")?;

        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let text = rendered_text(&terminal);
        assert!(text.contains("A Recorded Paper"));
        let expected_time = super::format_local_datetime(&timestamp, "%Y-%m-%d %H:%M");
        assert!(text.contains(&expected_time));
        assert!(!text.contains("UTC"));

        app.page = Page::Statistics;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let statistics = rendered_text(&terminal);
        assert!(statistics.contains("4 days"));
        assert!(statistics.contains("Monday"));
        Ok(())
    }

    #[test]
    fn collections_and_their_papers_render_as_selectable_lists()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let collection = CollectionSummary {
            id: 4,
            name: "Important Papers".into(),
            paper_count: 1,
            folder_path: Some("/tmp/Important Papers".into()),
        };
        let mut app = App {
            page: Page::Collections,
            collections: vec![collection.clone()],
            ..App::default()
        };
        let theme = Theme::load("nord")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        assert!(rendered_text(&terminal).contains("Important Papers"));

        app.active_collection = Some(collection);
        app.collection_papers.push(LibraryPaper {
            id: 8,
            title: "Paper In Collection".into(),
            authors: "Researcher".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("/tmp/paper.pdf".into()),
            file_size: Some(1024),
            reading_status: "unread".into(),
            is_favorite: false,
        });
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("paper"));
        assert!(!rendered.contains("paper.pdf"));
        assert!(rendered.contains("PDF available"));
        Ok(())
    }

    #[test]
    fn bookmarks_render_pdf_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App {
            page: Page::Bookmarks,
            bookmarks: vec![BookmarkSummary {
                id: 1,
                paper_id: 8,
                paper_title: "Bookmarked Research".into(),
                authors: "Ada Lovelace, Alan Turing".into(),
                year: Some("2026".into()),
                journal: Some("Terminal Studies".into()),
                doi: None,
                pdf_path: "/tmp/paper.pdf".into(),
                page: None,
                label: None,
            }],
            ..App::default()
        };
        let theme = Theme::load("nord")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("paper"));
        assert!(!rendered.contains("paper.pdf"));
        assert!(rendered.contains("Ada Lovelace, Alan Turing"));
        assert!(rendered.contains("2026"));
        assert!(rendered.contains("Terminal Studies"));
        Ok(())
    }

    #[test]
    fn bookmarks_render_the_workspace_filtered_list() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend)?;
        let mut app = App {
            page: Page::Bookmarks,
            workspace: AppWorkspaceState {
                workspace_query: "target".into(),
                ..AppWorkspaceState::default()
            },
            bookmarks: vec![
                BookmarkSummary {
                    id: 1,
                    paper_id: 1,
                    paper_title: "Target paper".into(),
                    authors: "Ada Lovelace".into(),
                    year: None,
                    journal: None,
                    doi: None,
                    pdf_path: "/tmp/target.pdf".into(),
                    page: None,
                    label: None,
                },
                BookmarkSummary {
                    id: 2,
                    paper_id: 2,
                    paper_title: "Unrelated paper".into(),
                    authors: "Alan Turing".into(),
                    year: None,
                    journal: None,
                    doi: None,
                    pdf_path: "/tmp/unrelated.pdf".into(),
                    page: None,
                    label: None,
                },
            ],
            ..App::default()
        };
        let theme = Theme::load("nord")?;
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Ada Lovelace"));
        assert!(!rendered.contains("Alan Turing"));
        Ok(())
    }

    #[test]
    fn local_workspaces_render_the_current_pdf_filename() -> Result<(), Box<dyn std::error::Error>>
    {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend)?;
        let theme = Theme::load("nord")?;
        let paper = LibraryPaper {
            id: 1,
            title: "Bibliographic Paper Title".into(),
            authors: "Researcher".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("/library/renamed-on-disk.pdf".into()),
            file_size: Some(1024),
            reading_status: "unread".into(),
            is_favorite: false,
        };
        let collection = CollectionSummary {
            id: 1,
            name: "Research".into(),
            paper_count: 1,
            folder_path: Some("/library/Research".into()),
        };
        let bookmark = BookmarkSummary {
            id: 1,
            paper_id: paper.id,
            paper_title: paper.title.clone(),
            authors: paper.authors.clone(),
            year: None,
            journal: None,
            doi: None,
            pdf_path: paper.pdf_path.clone().unwrap_or_default(),
            page: None,
            label: None,
        };
        let download = DownloadTask {
            id: "local".into(),
            title: paper.title.clone(),
            downloaded: 1024,
            total: Some(1024),
            paper_id: Some(paper.id),
            pdf_path: paper.pdf_path.clone(),
            status: DownloadStatus::Completed,
            remote_paper: None,
            failed_at: None,
        };
        let mut app = App {
            library: LibraryState {
                papers: vec![paper.clone()],
                ..LibraryState::default()
            },
            ..App::default()
        };

        let mut assert_filename = |app: &mut App| -> Result<(), Box<dyn std::error::Error>> {
            terminal.draw(|frame| render(frame, app, &theme))?;
            let rendered = rendered_text(&terminal);
            assert!(rendered.contains("renamed-on-disk"));
            assert!(!rendered.contains("renamed-on-disk.pdf"));
            assert!(!rendered.contains("Bibliographic Paper Title"));
            Ok(())
        };

        app.page = Page::Library;
        assert_filename(&mut app)?;

        app.page = Page::Collections;
        app.active_collection = Some(collection);
        app.collection_papers = vec![paper.clone()];
        assert_filename(&mut app)?;

        app.page = Page::Authors;
        app.active_author = Some(AuthorSummary {
            id: 1,
            name: "Researcher".into(),
            paper_count: 1,
        });
        app.author_papers = vec![paper.clone()];
        assert_filename(&mut app)?;

        app.page = Page::Notes;
        app.notes_papers = vec![paper.clone()];
        assert_filename(&mut app)?;

        app.page = Page::ReadingQueue;
        app.reading_queue_papers = vec![paper.clone()];
        assert_filename(&mut app)?;

        app.page = Page::Bookmarks;
        app.bookmarks = vec![bookmark];
        assert_filename(&mut app)?;

        app.page = Page::Downloads;
        app.downloads = vec![download];
        assert_filename(&mut app)?;
        Ok(())
    }

    fn sample_paper() -> Result<RemotePaper, Box<dyn std::error::Error>> {
        let published = Utc
            .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
            .single()
            .ok_or("invalid test date")?;
        Ok(RemotePaper {
            id: "https://arxiv.org/abs/2601.00001".into(),
            title: "Terminal Research Systems".into(),
            authors: vec!["Ada Lovelace".into(), "Alan Turing".into()],
            abstract_text: "A testable abstract for a paper detail view.".into(),
            published,
            updated: published,
            categories: vec!["cs.HC".into()],
            pdf_url: Some("https://arxiv.org/pdf/2601.00001".into()),
            doi: Some("10.1000/papr".into()),
            journal_ref: Some("Journal of Terminal Studies".into()),
        })
    }

    fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn test_project_activity_formatting_and_rendering() {
        assert_eq!(super::activity_kind("project_opened"), "Worked on");
        assert_eq!(super::activity_kind("project_created"), "Project created");
        assert_eq!(super::activity_kind("project_renamed"), "Project renamed");
        assert_eq!(super::activity_kind("project_deleted"), "Project deleted");
        assert_eq!(super::activity_kind("paper_created"), "Paper created");
        assert_eq!(super::activity_kind("paper_renamed"), "Paper renamed");
        assert_eq!(super::activity_kind("paper_deleted"), "Paper deleted");
    }

    #[test]
    fn test_timezone_display_resolution() {
        let now = chrono::Local::now();
        let tz = super::resolve_timezone_display(&now);
        // Ensure no bare numeric offset is displayed without parentheses
        assert!(
            !(tz.starts_with('+') || tz.starts_with('-')),
            "Bare numeric offset found without parentheses: {tz}"
        );
        if tz.contains(':') {
            assert!(
                tz.starts_with('(') && tz.ends_with(')'),
                "Offset must be enclosed in parentheses: {tz}"
            );
        }

        // Verify IANA mappings
        assert_eq!(
            super::iana_to_abbreviation("Asia/Kolkata", &now),
            Some("IST")
        );
        assert_eq!(super::iana_to_abbreviation("Asia/Tokyo", &now), Some("JST"));
        assert_eq!(super::iana_to_abbreviation("UTC", &now), Some("UTC"));

        let formatted = super::format_local_datetime(&chrono::Utc::now(), "%Y-%m-%d %H:%M");
        assert!(formatted.contains(&tz));
    }
}
