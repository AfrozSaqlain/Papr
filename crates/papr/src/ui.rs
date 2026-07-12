//! Ratatui rendering for the application shell.

use papr_core::{
    App, AppMode, DiscoveryStatus, DownloadStatus, LibraryPaper, Page, RemotePaper, Theme,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

const LOGO: &str = "[ P A P R ]";

/// Render the complete application for the current state.
pub fn render(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );
    if area.width < 58 || area.height < 18 {
        render_too_small(frame, area, theme);
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
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(32)])
        .split(rows[1]);
    render_sidebar(frame, columns[0], app, theme);
    render_content(frame, columns[1], app, theme);
    render_status(frame, rows[2], app, theme);

    match app.mode {
        AppMode::CommandPalette => render_palette(frame, app, theme),
        AppMode::Help => render_help(frame, theme),
        AppMode::PaperDetail => render_paper_detail(frame, app, theme),
        AppMode::NoteEdit => render_note_editor(frame, app, theme),
        AppMode::Prompt => render_metadata_prompt(frame, app, theme),
        AppMode::Normal | AppMode::Search => {}
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
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
        Paragraph::new("Ctrl+P  Commands")
            .alignment(Alignment::Right)
            .style(Style::default().fg(theme.muted)),
        shortcut,
    );
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items = Page::ALL
        .iter()
        .map(|page| ListItem::new(format!("  {}", page.title())));
    let list = List::new(items)
        .block(
            Block::default()
                .title(" NAVIGATE ")
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(theme.border)),
        )
        .style(Style::default().fg(theme.muted))
        .highlight_style(
            Style::default()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("›");
    let mut state = ListState::default().with_selected(Some(app.sidebar_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_content(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let inset = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    if app.page == Page::Dashboard {
        render_dashboard(frame, inset, app, theme);
    } else if app.page == Page::Discover {
        render_discover(frame, inset, app, theme);
    } else if app.page == Page::Library {
        render_library(frame, inset, app, theme);
    } else if app.page == Page::Downloads {
        render_downloads(frame, inset, app, theme);
    } else if app.page == Page::Collections {
        render_collections(frame, inset, app, theme);
    } else if app.page == Page::Bookmarks {
        render_organization(frame, inset, app, theme);
    } else if app.page == Page::Notes {
        frame.render_widget(
            Paragraph::new("Select a paper in Library or Discover and press n to edit its note.")
                .style(Style::default().fg(theme.muted)),
            inset,
        );
    } else if app.page == Page::History {
        render_history(frame, inset, app, theme);
    } else if app.page == Page::Statistics {
        render_statistics(frame, inset, app, theme);
    } else if app.page == Page::Settings {
        render_settings(frame, inset, app, theme);
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

fn render_collections(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    if let Some(collection) = &app.active_collection {
        render_collection_papers(frame, area, app, collection, theme);
        return;
    }
    if app.collections.is_empty() {
        frame.render_widget(
            Paragraph::new("No collections yet. Select a paper and press s to create one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let items = app.collections.iter().map(|collection| {
        ListItem::new(vec![
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
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" COLLECTIONS - ENTER TO VIEW PAPERS ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.collection_selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_collection_papers(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    collection: &papr_core::CollectionSummary,
    theme: &Theme,
) {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Collections / ", Style::default().fg(theme.muted)),
            Span::styled(
                &collection.name,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   h/Esc back   Enter/p open PDF",
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[0],
    );
    if app.collection_papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No papers are assigned to this collection.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let items = app.collection_papers.iter().map(|paper| {
        let availability = if paper.pdf_path.is_some() {
            "PDF available"
        } else {
            "Metadata only"
        };
        ListItem::new(vec![
            Line::styled(
                &paper.title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!("{}  |  {}", library_metadata(paper), availability),
                Style::default().fg(theme.muted),
            ),
            Line::raw(""),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", app.collection_papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.collection_paper_selected));
    frame.render_stateful_widget(list, rows[1], &mut state);
}

fn render_settings(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(4), Constraint::Min(4)]).split(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("Configuration", Style::default().fg(theme.text).add_modifier(Modifier::BOLD)),
            Line::styled(
                "Theme, library paths, PDF viewer, and plugin allowlist are loaded from config.toml.",
                Style::default().fg(theme.muted),
            ),
        ]),
        rows[0],
    );
    let items = if app.plugins.is_empty() {
        vec![ListItem::new("No valid plugins discovered")]
    } else {
        app.plugins
            .iter()
            .map(|plugin| {
                let state = if plugin.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                ListItem::new(vec![
                    Line::styled(
                        format!("{}  v{}", plugin.name, plugin.version),
                        Style::default().fg(if plugin.enabled {
                            theme.success
                        } else {
                            theme.text
                        }),
                    ),
                    Line::styled(
                        format!("{}  |  {}  |  {}", plugin.id, state, plugin.description),
                        Style::default().fg(theme.muted),
                    ),
                ])
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(
                    " PLUGINS  {} valid  {} invalid ",
                    app.plugins.len(),
                    app.plugin_diagnostics
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        ),
        rows[1],
    );
}

fn render_history(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items = app.dashboard.recent_activity.iter().map(|activity| {
        ListItem::new(vec![
            Line::styled(
                &activity.label,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!(
                    "{}  {}",
                    activity_kind(&activity.kind),
                    activity.occurred_at.format("%Y-%m-%d %H:%M UTC")
                ),
                Style::default().fg(theme.muted),
            ),
            Line::raw(""),
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

fn render_statistics(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
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
                    "Average reading time: {} min",
                    reading.average_reading_seconds / 60
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
        "paper_opened" => "Paper opened",
        "pdf_opened" => "PDF opened",
        "note_opened" => "Note opened",
        "search" => "Search",
        "downloaded" => "Downloaded",
        "bookmarked" => "Bookmark changed",
        "tagged" => "Legacy organization event",
        "collected" => "Added to collection",
        _ => kind,
    }
}

fn render_organization(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items: Vec<ListItem<'_>> = match app.page {
        Page::Bookmarks => app
            .bookmarks
            .iter()
            .map(|item| {
                let position = item
                    .page
                    .map_or_else(String::new, |page| format!("  page {page}"));
                ListItem::new(format!("{}{}", item.paper_title, position))
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
        frame.render_widget(
            List::new(items)
                .block(
                    Block::default()
                        .title(format!(" {} ", app.page.title().to_ascii_uppercase()))
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme.border)),
                )
                .style(Style::default().fg(theme.text)),
            area,
        );
    }
}

fn render_note_editor(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = centered(82, 24, frame.area());
    frame.render_widget(Clear, area);
    let body = app
        .note_editor
        .as_ref()
        .map_or("", |note| note.body.as_str());
    let content = if app.note_preview {
        markdown_preview(body, theme)
    } else {
        vec![Line::raw(body)]
    };
    let title = if app.note_preview {
        " MARKDOWN PREVIEW - TAB TO EDIT "
    } else {
        " MARKDOWN NOTE - AUTOSAVED - TAB TO PREVIEW "
    };
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent)),
            ),
        area,
    );
    if app.note_preview {
        return;
    }
    let column = u16::try_from(body.lines().last().map_or(0, |line| line.chars().count()))
        .unwrap_or(u16::MAX);
    let row = u16::try_from(body.lines().count().saturating_sub(1)).unwrap_or(u16::MAX);
    let cursor_x = area.x.saturating_add(1).saturating_add(column);
    let cursor_y = area.y.saturating_add(1).saturating_add(row);
    frame.set_cursor_position((
        cursor_x.min(area.right().saturating_sub(2)),
        cursor_y.min(area.bottom().saturating_sub(2)),
    ));
}

fn markdown_preview<'a>(body: &'a str, theme: &Theme) -> Vec<Line<'a>> {
    body.lines()
        .map(|line| {
            if line.starts_with('#') {
                Line::styled(
                    line.trim_start_matches('#').trim(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else if line.starts_with(char::from(96)) || line.starts_with("    ") {
                Line::styled(line, Style::default().fg(theme.success).bg(theme.surface))
            } else if line.starts_with("- ") || line.starts_with("* ") {
                Line::from(vec![
                    Span::styled("* ", Style::default().fg(theme.secondary)),
                    Span::styled(&line[2..], Style::default().fg(theme.text)),
                ])
            } else if let Some(quote) = line.strip_prefix("> ") {
                Line::from(vec![
                    Span::styled("| ", Style::default().fg(theme.warning)),
                    Span::styled(
                        quote,
                        Style::default()
                            .fg(theme.muted)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ])
            } else {
                Line::styled(line, Style::default().fg(theme.text))
            }
        })
        .collect()
}

fn render_metadata_prompt(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(prompt) = &app.metadata_prompt else {
        return;
    };
    let title = " ADD TO COLLECTION ";
    let area = centered(60, 3, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("> {}", prompt.value))
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.secondary)),
            ),
        area,
    );
    let column = u16::try_from(prompt.value.chars().count()).unwrap_or(u16::MAX);
    frame.set_cursor_position((
        area.x.saturating_add(3).saturating_add(column),
        area.y.saturating_add(1),
    ));
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
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
    if app.library.papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No local papers indexed")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let items = app.library.papers.iter().map(|paper| {
        ListItem::new(vec![
            Line::styled(
                &paper.title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(library_metadata(paper), Style::default().fg(theme.muted)),
            Line::raw(""),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} PAPERS ", app.library.papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.library.selected));
    frame.render_stateful_widget(list, rows[1], &mut state);
}

fn library_metadata(paper: &LibraryPaper) -> String {
    let authors = if paper.authors.is_empty() {
        "Unknown authors"
    } else {
        &paper.authors
    };
    let size = paper
        .file_size
        .map_or_else(|| "metadata only".to_owned(), format_bytes);
    format!("{authors}  |  {}  |  {size}", paper.reading_status)
}

fn render_downloads(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    if app.downloads.is_empty() {
        frame.render_widget(
            Paragraph::new("No downloads yet. Press d on an arXiv paper to start one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let items = app.downloads.iter().map(|download| {
        let (label, color) = match &download.status {
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
            DownloadStatus::Completed => ("Completed".to_owned(), theme.success),
            DownloadStatus::Failed(error) => (format!("Failed: {error}"), theme.error),
        };
        ListItem::new(vec![
            Line::styled(
                &download.title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(label, Style::default().fg(color)),
            Line::raw(""),
        ])
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
    let mut state = ListState::default().with_selected(Some(app.download_selected));
    frame.render_stateful_widget(list, area, &mut state);
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

fn render_discover(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(area);
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
        rows[0],
    );

    match &app.discovery.status {
        DiscoveryStatus::Idle => render_discover_empty(frame, rows[1], theme),
        DiscoveryStatus::Loading => {
            frame.render_widget(
                Paragraph::new("Searching arXiv...")
                    .style(Style::default().fg(theme.accent))
                    .alignment(Alignment::Center),
                rows[1],
            );
        }
        DiscoveryStatus::Error(error) => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("Search failed", Style::default().fg(theme.error)),
                    Line::raw(""),
                    Line::styled(error, Style::default().fg(theme.muted)),
                    Line::raw(""),
                    Line::styled("Press r to retry", Style::default().fg(theme.text)),
                ])
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
                rows[1],
            );
        }
        DiscoveryStatus::Ready if app.discovery.results.is_empty() => {
            frame.render_widget(
                Paragraph::new("No papers found. Try a broader query.")
                    .style(Style::default().fg(theme.muted))
                    .alignment(Alignment::Center),
                rows[1],
            );
        }
        DiscoveryStatus::Ready => render_search_results(frame, rows[1], app, theme),
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

fn render_search_results(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items = app.discovery.results.iter().map(|paper| {
        let meta = format!(
            "{}  |  {}  |  {}",
            paper.published.format("%Y-%m-%d"),
            compact_authors(paper),
            paper
                .categories
                .first()
                .map_or("uncategorized", String::as_str)
        );
        ListItem::new(vec![
            Line::styled(
                &paper.title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(meta, Style::default().fg(theme.muted)),
            Line::raw(""),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} RESULTS ", app.discovery.results.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.discovery.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn compact_authors(paper: &RemotePaper) -> String {
    match paper.authors.as_slice() {
        [] => "Unknown authors".to_owned(),
        [author] => author.clone(),
        [first, second] => format!("{first}, {second}"),
        [first, ..] => format!("{first} et al."),
    }
}

fn render_paper_detail(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(paper) = app.discovery.results.get(app.discovery.selected) else {
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
    frame.render_widget(
        Paragraph::new(paper_detail_lines(paper, theme))
            .wrap(Wrap { trim: true })
            .scroll((app.discovery.detail_scroll, 0)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new("j/k scroll  h back  d download  n note  t tag  s collection  B bookmark")
            .style(Style::default().fg(theme.muted)),
        rows[2],
    );
}

fn paper_detail_lines<'a>(paper: &'a RemotePaper, theme: &Theme) -> Vec<Line<'a>> {
    let doi = paper.doi.as_deref().unwrap_or("Not available");
    let journal = paper.journal_ref.as_deref().unwrap_or("Not available");
    let pdf = paper.pdf_url.as_deref().unwrap_or("Not available");
    vec![
        Line::styled(
            &paper.title,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(paper.author_line(), Style::default().fg(theme.secondary)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Published  ", Style::default().fg(theme.muted)),
            Span::styled(
                paper.published.format("%B %d, %Y").to_string(),
                Style::default().fg(theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Updated    ", Style::default().fg(theme.muted)),
            Span::styled(
                paper.updated.format("%B %d, %Y").to_string(),
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

fn render_dashboard(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
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
        ("UNREAD", app.dashboard.unread, theme.secondary),
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

fn render_today_research(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let today = match &app.today_status {
        DiscoveryStatus::Loading => vec![ListItem::new("Loading today's arXiv papers...")],
        DiscoveryStatus::Error(_) => vec![ListItem::new("Today's feed is unavailable")],
        _ if app.today_papers.is_empty() => vec![ListItem::new("No new papers loaded")],
        _ => app
            .today_papers
            .iter()
            .take(5)
            .map(|paper| {
                ListItem::new(vec![
                    Line::styled(
                        &paper.title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        format!(
                            "{}  {}",
                            compact_authors(paper),
                            paper.published.format("%Y-%m-%d")
                        ),
                        Style::default().fg(theme.muted),
                    ),
                ])
            })
            .collect(),
    };
    frame.render_widget(
        List::new(today).block(
            Block::default()
                .title(" TODAY'S NEW PAPERS ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        ),
        area,
    );
}

fn render_dashboard_details(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
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
                format!("Collections  {}", app.dashboard.collections),
                Style::default().fg(theme.muted),
            ),
        ]),
        storage,
    );
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let line = Line::from(vec![
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
    ]);
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

fn render_palette(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = centered(64, 9, frame.area());
    frame.render_widget(Clear, area);
    let query = if app.palette_query.is_empty() {
        "Type a command…"
    } else {
        &app.palette_query
    };
    let content = vec![
        Line::styled(format!("> {query}"), Style::default().fg(theme.text)),
        Line::raw(""),
        Line::styled("Open Dashboard", Style::default().fg(theme.accent)),
        Line::styled("Open Library", Style::default().fg(theme.muted)),
        Line::styled("Quit papr", Style::default().fg(theme.muted)),
    ];
    frame.render_widget(
        Paragraph::new(content).block(
            Block::default()
                .title(" COMMANDS ")
                .borders(Borders::ALL)
                .style(Style::default().bg(theme.surface))
                .border_style(Style::default().fg(theme.accent)),
        ),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, theme: &Theme) {
    let area = centered(64, 18, frame.area());
    frame.render_widget(Clear, area);
    let help = "j / k      Move selection\nEnter      Open selection\nh / l      Back / open\nCtrl+p     Command palette\n/          Search arXiv\np          Open local PDF\nd          Download\nn          Notes\ns          Collection\nB          Bookmark\nq          Close / quit\n?          Toggle this help";
    frame.render_widget(
        Paragraph::new(help)
            .style(Style::default().fg(theme.text))
            .block(
                Block::default()
                    .title(" KEYBOARD REFERENCE ")
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.surface))
                    .border_style(Style::default().fg(theme.secondary)),
            ),
        area,
    );
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use papr_core::{
        ActivityItem, App, AppMode, CollectionSummary, DiscoveryStatus, DownloadStatus,
        DownloadTask, LibraryPaper, Page, RemotePaper, Theme,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::render;

    #[test]
    fn dashboard_renders_at_minimum_size() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        let app = App::default();
        let theme = Theme::load("catppuccin")?;
        terminal.draw(|frame| render(frame, &app, &theme))?;
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

        terminal.draw(|frame| render(frame, &app, &theme))?;
        assert!(rendered_text(&terminal).contains("Terminal Research Systems"));

        app.mode = AppMode::PaperDetail;
        terminal.draw(|frame| render(frame, &app, &theme))?;
        let detail = rendered_text(&terminal);
        assert!(detail.contains("ABSTRACT"));
        assert!(detail.contains("10.1000/papr"));
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
            pdf_path: Some("paper.pdf".into()),
            file_size: Some(2048),
            reading_status: "unread".into(),
            is_favorite: false,
        });
        let theme = Theme::load("gruvbox")?;
        terminal.draw(|frame| render(frame, &app, &theme))?;
        assert!(rendered_text(&terminal).contains("A Local Research Paper"));

        app.page = Page::Downloads;
        app.downloads.push(DownloadTask {
            id: "arxiv:test".into(),
            title: "A Streaming Download".into(),
            downloaded: 1024,
            total: Some(2048),
            status: DownloadStatus::Running,
        });
        terminal.draw(|frame| render(frame, &app, &theme))?;
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

        terminal.draw(|frame| render(frame, &app, &theme))?;
        assert!(rendered_text(&terminal).contains("A Recorded Paper"));

        app.page = Page::Statistics;
        terminal.draw(|frame| render(frame, &app, &theme))?;
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
        };
        let mut app = App {
            page: Page::Collections,
            collections: vec![collection.clone()],
            ..App::default()
        };
        let theme = Theme::load("nord")?;
        terminal.draw(|frame| render(frame, &app, &theme))?;
        assert!(rendered_text(&terminal).contains("Important Papers"));

        app.active_collection = Some(collection);
        app.collection_papers.push(LibraryPaper {
            id: 8,
            title: "Paper In Collection".into(),
            authors: "Researcher".into(),
            doi: None,
            pdf_path: Some("/tmp/paper.pdf".into()),
            file_size: Some(1024),
            reading_status: "unread".into(),
            is_favorite: false,
        });
        terminal.draw(|frame| render(frame, &app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Paper In Collection"));
        assert!(rendered.contains("PDF available"));
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
}
