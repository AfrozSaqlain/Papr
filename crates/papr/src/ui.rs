//! Ratatui rendering for the application shell.

use papr_core::{
    App, AppMode, DiscoveryStatus, DownloadStatus, LibraryPaper, Page, PromptKind, RemotePaper,
    Theme,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
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
    } else if matches!(app.page, Page::Collections | Page::Tags | Page::Bookmarks) {
        render_organization(frame, inset, app, theme);
    } else if app.page == Page::Notes {
        frame.render_widget(
            Paragraph::new("Select a paper in Library or Discover and press n to edit its note.")
                .style(Style::default().fg(theme.muted)),
            inset,
        );
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

fn render_organization(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    let items: Vec<ListItem<'_>> = match app.page {
        Page::Collections => app
            .collections
            .iter()
            .map(|item| ListItem::new(format!("{}  ({} papers)", item.name, item.paper_count)))
            .collect(),
        Page::Tags => app
            .tags
            .iter()
            .map(|item| ListItem::new(format!("#{}  ({} papers)", item.name, item.paper_count)))
            .collect(),
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
    let title = match prompt.kind {
        PromptKind::Tag => " ADD TAG ",
        PromptKind::Collection => " ADD TO COLLECTION ",
    };
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
        "Search title, author, abstract, category, DOI..."
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
        Constraint::Length(2),
        Constraint::Length(5),
        Constraint::Min(5),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Today in Research",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );
    let cards = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(rows[1]);
    let data = [
        ("LIBRARY", app.stats.papers, theme.accent),
        ("QUEUE", app.stats.queued, theme.warning),
        ("DOWNLOADED", app.stats.downloaded, theme.success),
        ("FAVORITES", app.stats.favorites, theme.secondary),
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
    let lower = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
        .spacing(1)
        .split(rows[2]);
    let activity = Paragraph::new(vec![
        Line::styled("No recent activity", Style::default().fg(theme.text)),
        Line::styled(
            "Imported papers and reading sessions will appear here.",
            Style::default().fg(theme.muted),
        ),
    ])
    .block(
        Block::default()
            .title(" RECENT ACTIVITY ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );
    frame.render_widget(activity, lower[0]);
    let progress_area = lower[1].inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 2,
    });
    frame.render_widget(
        Block::default()
            .title(" WEEKLY GOAL ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        lower[1],
    );
    frame.render_widget(
        Gauge::default()
            .ratio(0.0)
            .label("0 / 5 papers")
            .gauge_style(Style::default().fg(theme.success).bg(theme.surface)),
        Rect::new(progress_area.x, progress_area.y, progress_area.width, 1),
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
    let help = "j / k      Move selection\nEnter      Open section\nh / l      Back / open\nCtrl+p     Command palette\n/          Search (Milestone 2)\np          Open PDF\nd          Download\nb          Copy BibTeX\nf          Favorite\nn          Notes\nt          Tag\nm          Mark read\nq          Close / quit\n?          Toggle this help";
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
        App, AppMode, DiscoveryStatus, DownloadStatus, DownloadTask, LibraryPaper, Page,
        RemotePaper, Theme,
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
