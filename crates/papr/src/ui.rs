//! Ratatui rendering for the application shell.

use crate::build_config_editor_view;
use papr_core::{
    App, AppMode, DeletionTarget, DiscoveryStatus, DownloadStatus, LibraryPaper, Page, RemotePaper, Theme,
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
pub fn render(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
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
        AppMode::ConfirmDelete => render_delete_confirmation(frame, app, theme),
        AppMode::Normal | AppMode::Search | AppMode::WorkspaceSearch => {}
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
        Paragraph::new("Ctrl+P  Commands")
            .alignment(Alignment::Right)
            .style(Style::default().fg(theme.muted)),
        shortcut,
    );
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
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
    frame.render_stateful_widget(list, area, &mut state);
    app.sidebar_scroll = state.offset();
}

fn render_content(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let inset = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let mut workspace_area = inset;
    if matches!(
        app.page,
        Page::Library | Page::Downloads | Page::Collections | Page::Authors | Page::Bookmarks
    ) {
        let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(inset);
        render_workspace_search_bar(frame, rows[0], app, theme);
        workspace_area = rows[1];
    }

    if app.page == Page::Dashboard {
        render_dashboard(frame, inset, app, theme);
    } else if app.page == Page::Discover {
        render_discover(frame, inset, app, theme);
    } else if app.page == Page::Library {
        render_library(frame, workspace_area, app, theme);
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
        .title(" Local Search (Ctrl+/) ");
    
    let mut text = app.workspace_query.clone();
    if text.is_empty() && app.mode != AppMode::WorkspaceSearch {
        text = "Type to filter...".to_owned();
    }
    frame.render_widget(Paragraph::new(text).style(style).block(block), area);
    
    if app.mode == AppMode::WorkspaceSearch {
        let cursor_offset = app
            .workspace_query
            .chars()
            .take(app.workspace_query_cursor)
            .count();
        frame.set_cursor_position((
            area.x.saturating_add(1).saturating_add(u16::try_from(cursor_offset).unwrap_or(0)),
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
            Paragraph::new("No collections yet. Select a paper and press s to create one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let items = collections.iter().map(|item| {
        use papr_core::CollectionSearchItem;
        match item {
            CollectionSearchItem::Collection(collection) => {
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
            }
            CollectionSearchItem::Paper(paper, _) => {
                ListItem::new(vec![
                    Line::styled(
                        format!("  {}", paper.title),
                        Style::default().fg(theme.text),
                    ),
                    Line::styled(
                        format!("    {}", paper.authors),
                        Style::default().fg(theme.muted),
                    ),
                ])
            }
        }
    });
    let list = List::new(items)
        .block(
            Block::default()
                .title(" COLLECTIONS - ENTER VIEW  c NEW  R RENAME ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(Some(app.collection_selected))
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
    if papers.is_empty() {
        frame.render_widget(
            Paragraph::new("No papers are assigned to this collection.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted)),
            rows[1],
        );
        return;
    }
    let items = papers.iter().map(|paper| {
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
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(Some(app.collection_paper_selected))
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
        ListItem::new(vec![
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
        .with_selected(Some(app.author_selected))
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
                "   h/Esc back   Enter/p open PDF",
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
    let items = papers.iter().map(|paper| {
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
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(Some(app.author_paper_selected))
        .with_offset(app.author_paper_scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.author_paper_scroll = state.offset();
}

fn render_settings(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let title = if app.config_editor_focused {
        if app.config_editor_insert_mode {
            " CONFIG.TOML - INSERT "
        } else if app.config_editor_command.is_some() {
            " CONFIG.TOML - COMMAND "
        } else {
            " CONFIG.TOML - NORMAL "
        }
    } else {
        " CONFIG.TOML "
    };

    let border_style = if app.config_editor_focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.border)
    };

    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).split(area);
    let height = chunks[0].height.saturating_sub(2) as usize;
    let wrap_width = chunks[0].width.saturating_sub(6) as usize;
    app.config_editor_wrap_width = wrap_width.max(1);
    app.config_editor_viewport_height = height;

    let view = build_config_editor_view(
        &app.config_editor_text,
        app.config_editor_cursor,
        app.config_editor_wrap_width,
        height,
        &mut app.config_editor_scroll,
    );

    let displayed = view
        .lines
        .iter()
        .skip(app.config_editor_scroll)
        .take(height)
        .map(|line| {
            let (prefix, content) = line.split_at(4.min(line.len()));
            Line::from(vec![
                Span::styled(prefix.to_owned(), Style::default().fg(theme.muted)),
                Span::styled(content.to_owned(), Style::default().fg(theme.text)),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(displayed)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(border_style),
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.surface)),
        chunks[0],
    );

    let status_line = if let Some(ref err) = app.config_editor_error {
        Line::styled(err.clone(), Style::default().fg(theme.error))
    } else if let Some(ref cmd) = app.config_editor_command {
        Line::styled(format!(":{}", cmd), Style::default().fg(theme.text))
    } else {
        Line::styled(
            if app.config_editor_insert_mode {
                "-- INSERT --"
            } else {
                "-- NORMAL --"
            },
            Style::default().fg(theme.muted),
        )
    };
    frame.render_widget(Paragraph::new(status_line), chunks[1]);

    if app.config_editor_focused {
        let screen_x = chunks[0]
            .x
            .saturating_add(5)
            .saturating_add(u16::try_from(view.cursor_col).unwrap_or(0));
        let screen_y = chunks[0]
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(view.cursor_row.saturating_sub(app.config_editor_scroll)).unwrap_or(0));
        frame.set_cursor_position((screen_x, screen_y));
    }
}

fn render_history(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
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

fn render_organization(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let items: Vec<ListItem<'_>> = match app.page {
        Page::Bookmarks => app
            .bookmarks
            .iter()
            .map(|item| {
                let mut metadata = Vec::new();
                if !item.authors.is_empty() {
                    metadata.push(item.authors.clone());
                }
                if let Some(year) = &item.year {
                    metadata.push(year.clone());
                }
                if let Some(journal) = &item.journal {
                    metadata.push(journal.clone());
                } else if let Some(doi) = &item.doi {
                    metadata.push(format!("DOI {doi}"));
                }
                if let Some(page) = item.page {
                    metadata.push(format!("page {page}"));
                }
                if metadata.is_empty() {
                    metadata.push("Local PDF".into());
                }
                ListItem::new(vec![
                    Line::styled(
                        &item.paper_title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(metadata.join("  |  "), Style::default().fg(theme.muted)),
                    Line::raw(""),
                ])
            })
            .collect(),
        Page::Notes => app
            .filtered_notes_papers()
            .iter()
            .map(|paper| {
                ListItem::new(vec![
                    Line::styled(
                        &paper.title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(library_metadata(paper), Style::default().fg(theme.muted)),
                    Line::raw(""),
                ])
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
            .with_selected(Some(selected_idx))
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
    let area = centered(82, 24, frame.area());
    frame.render_widget(Clear, area);
    let body = app
        .note_editor
        .as_ref()
        .map_or("", |note| note.body.as_str());
    let content = if app.note_preview {
        markdown_preview(body, theme)
    } else {
        body.split('\n').map(|l| Line::raw(l.to_string())).collect()
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
    let text_before_cursor = &body[..app.note_editor.as_ref().unwrap().cursor];
    let row = u16::try_from(text_before_cursor.split('\n').count().saturating_sub(1)).unwrap_or(0);
    let column = u16::try_from(
        text_before_cursor
            .split('\n')
            .last()
            .unwrap_or("")
            .chars()
            .count(),
    )
    .unwrap_or(0);
    let cursor_x = area.x.saturating_add(1).saturating_add(column);
    let cursor_y = area.y.saturating_add(1).saturating_add(row);
    frame.set_cursor_position((
        cursor_x.min(area.right().saturating_sub(2)),
        cursor_y.min(area.bottom().saturating_sub(2)),
    ));
}

fn parse_inline<'a>(mut text: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    while !text.is_empty() {
        let math_idx = text.find('$');
        let code_idx = text.find('`');
        let bold_idx = text.find("**");
        let italic_idx = text.find('*');
        let link_idx = text.find('[');
        
        let mut first: Option<(usize, &str)> = None;
        if let Some(idx) = math_idx {
            if first.map_or(true, |(i, _)| idx < i) {
                first = Some((idx, "$"));
            }
        }
        if let Some(idx) = code_idx {
            if first.map_or(true, |(i, _)| idx < i) {
                first = Some((idx, "`"));
            }
        }
        if let Some(idx) = bold_idx {
            if first.map_or(true, |(i, _)| idx < i) {
                first = Some((idx, "**"));
            }
        }
        if let Some(idx) = italic_idx {
            if first.map_or(true, |(i, _)| idx < i) {
                if bold_idx == Some(idx) {
                    first = Some((idx, "**"));
                } else {
                    first = Some((idx, "*"));
                }
            }
        }
        if let Some(idx) = link_idx {
            if first.map_or(true, |(i, _)| idx < i) {
                first = Some((idx, "["));
            }
        }
        
        if let Some((idx, marker)) = first {
            if idx > 0 {
                spans.push(Span::styled(&text[..idx], Style::default().fg(theme.text)));
            }
            let rest = &text[idx + marker.len()..];
            match marker {
                "$" => {
                    if let Some(close_idx) = rest.find('$') {
                        let math_text = &rest[..close_idx];
                        spans.push(Span::styled(
                            math_text,
                            Style::default().fg(theme.warning).add_modifier(Modifier::ITALIC)
                        ));
                        text = &rest[close_idx + 1..];
                    } else {
                        spans.push(Span::styled(marker, Style::default().fg(theme.text)));
                        text = rest;
                    }
                }
                "`" => {
                    if let Some(close_idx) = rest.find('`') {
                        let code_text = &rest[..close_idx];
                        spans.push(Span::styled(
                            code_text,
                            Style::default().fg(theme.success).bg(theme.surface)
                        ));
                        text = &rest[close_idx + 1..];
                    } else {
                        spans.push(Span::styled(marker, Style::default().fg(theme.text)));
                        text = rest;
                    }
                }
                "**" => {
                    if let Some(close_idx) = rest.find("**") {
                        let bold_text = &rest[..close_idx];
                        spans.push(Span::styled(
                            bold_text,
                            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
                        ));
                        text = &rest[close_idx + 2..];
                    } else {
                        spans.push(Span::styled(marker, Style::default().fg(theme.text)));
                        text = rest;
                    }
                }
                "*" => {
                    if let Some(close_idx) = rest.find('*') {
                        let italic_text = &rest[..close_idx];
                        spans.push(Span::styled(
                            italic_text,
                            Style::default().fg(theme.text).add_modifier(Modifier::ITALIC)
                        ));
                        text = &rest[close_idx + 1..];
                    } else {
                        spans.push(Span::styled(marker, Style::default().fg(theme.text)));
                        text = rest;
                    }
                }
                "[" => {
                    if let Some(close_text_idx) = rest.find(']') {
                        let link_text = &rest[..close_text_idx];
                        let after_text = &rest[close_text_idx + 1..];
                        if after_text.starts_with('(') {
                            if let Some(close_url_idx) = after_text.find(')') {
                                let url = &after_text[1..close_url_idx];
                                spans.push(Span::styled(
                                    link_text,
                                    Style::default().fg(theme.accent).add_modifier(Modifier::UNDERLINED)
                                ));
                                spans.push(Span::styled(
                                    format!(" ({})", url),
                                    Style::default().fg(theme.muted)
                                ));
                                text = &after_text[close_url_idx + 1..];
                                continue;
                            }
                        }
                    }
                    spans.push(Span::styled(marker, Style::default().fg(theme.text)));
                    text = rest;
                }
                _ => {
                    text = rest;
                }
            }
        } else {
            spans.push(Span::styled(text, Style::default().fg(theme.text)));
            break;
        }
    }
    spans
}

fn markdown_preview<'a>(body: &'a str, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut in_math_block = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::styled(
                "── Code Block ──────────────────────────────────",
                Style::default().fg(theme.muted),
            ));
            continue;
        }
        if in_code_block {
            lines.push(Line::styled(
                line,
                Style::default().fg(theme.success).bg(theme.surface),
            ));
            continue;
        }
        if trimmed.starts_with("$$") {
            in_math_block = !in_math_block;
            lines.push(Line::styled(
                "── Display Math ─────────────────────────────────",
                Style::default().fg(theme.muted),
            ));
            continue;
        }
        if in_math_block {
            lines.push(Line::styled(
                line,
                Style::default().fg(theme.warning).add_modifier(Modifier::ITALIC),
            ));
            continue;
        }
        if let Some(h) = line.strip_prefix("# ") {
            lines.push(Line::styled(
                format!("# {}", h.trim()),
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            lines.push(Line::styled(
                format!("## {}", h.trim()),
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(h) = line.strip_prefix("### ") {
            lines.push(Line::styled(
                format!("### {}", h.trim()),
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(quote) = line.strip_prefix("> ") {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.warning))];
            spans.extend(parse_inline(quote, theme));
            lines.push(Line::from(spans));
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            let mut spans = vec![Span::styled("• ", Style::default().fg(theme.secondary))];
            spans.extend(parse_inline(item, theme));
            lines.push(Line::from(spans));
            continue;
        }
        if let Some(item) = line.strip_prefix("* ") {
            let mut spans = vec![Span::styled("• ", Style::default().fg(theme.secondary))];
            spans.extend(parse_inline(item, theme));
            lines.push(Line::from(spans));
            continue;
        }
        let mut is_ordered_list = false;
        if let Some(dot_idx) = trimmed.find(". ") {
            if trimmed[..dot_idx].chars().all(|c| c.is_ascii_digit()) {
                is_ordered_list = true;
            }
        }
        if is_ordered_list {
            let dot_idx = trimmed.find(". ").unwrap();
            let prefix = &trimmed[..dot_idx + 2];
            let rest = &trimmed[dot_idx + 2..];
            let mut spans = vec![Span::styled(prefix, Style::default().fg(theme.secondary))];
            spans.extend(parse_inline(rest, theme));
            lines.push(Line::from(spans));
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let parts: Vec<&str> = trimmed.split('|').collect();
            let mut spans = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                if i > 0 && i < parts.len() - 1 {
                    spans.push(Span::styled("│", Style::default().fg(theme.accent)));
                    let text = part.trim();
                    if text.chars().all(|c| c == '-' || c == ':') && !text.is_empty() {
                        spans.push(Span::styled(part.to_string(), Style::default().fg(theme.muted)));
                    } else {
                        spans.extend(parse_inline(part, theme));
                    }
                }
            }
            spans.push(Span::styled("│", Style::default().fg(theme.accent)));
            lines.push(Line::from(spans));
            continue;
        }
        lines.push(Line::from(parse_inline(line, theme)));
    }
    lines
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
        DeletionTarget::Paper { title, .. } => (
            " CONFIRM DELETE PDF ",
            "Are you sure you want to permanently delete this PDF file from your disk?",
            title.as_str(),
        ),
        DeletionTarget::Collection { name, .. } => (
            " CONFIRM DELETE COLLECTION ",
            "Are you sure you want to permanently delete this collection (subdirectory) and ALL of its contents?",
            name.as_str(),
        ),
    };

    let height = 12;
    let width = 64;
    let area = centered(width, height, frame.area());
    frame.render_widget(Clear, area);

    let lines = vec![
        Line::raw(""),
        Line::styled(message, Style::default().fg(theme.text)),
        Line::raw(""),
        Line::styled(format!("  \"{}\"", item_name), Style::default().fg(theme.warning).add_modifier(Modifier::BOLD)),
        Line::raw(""),
        Line::styled("Press [y/Enter] to confirm, or [n/Esc/q] to cancel.", Style::default().fg(theme.muted)),
    ];

    let block = Block::default()
        .title(Line::styled(title, Style::default().fg(theme.error).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));
        
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), area);
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
        " RENAME COLLECTION "
    } else if creating {
        " CREATE COLLECTION "
    } else {
        " CHOOSE OR CREATE COLLECTION "
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
        chunks.len() as u16
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
    if has_current {
        lines.push(Line::styled(
            format!(
                "Current collection: {}",
                prompt.current_collection.as_ref().unwrap()
            ),
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
            "Or select an existing collection:",
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
    let items = papers.iter().map(|paper| {
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
                .title(format!(" {} PAPERS ", papers.len()))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(Style::default().bg(theme.surface).fg(theme.accent))
        .highlight_symbol("> ");
    let mut state = ListState::default()
        .with_selected(Some(app.library.selected))
        .with_offset(app.library.scroll);
    frame.render_stateful_widget(list, rows[1], &mut state);
    app.library.scroll = state.offset();
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
    let items = downloads.iter().map(|download| {
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
            DownloadStatus::Failed(_) => ("Failed".to_owned(), theme.error),
        };

        let paper = if let Some(paper_id) = download.paper_id {
            app.library.papers.iter().find(|p| p.id == paper_id)
        } else if let Some(pdf_path) = &download.pdf_path {
            app.library.papers.iter().find(|p| p.pdf_path.as_ref() == Some(pdf_path))
        } else {
            None
        };

        let (title, meta_str) = if let Some(paper) = paper {
            (paper.title.clone(), library_metadata(paper))
        } else {
            (download.title.clone(), "Processing metadata...".to_owned())
        };

        ListItem::new(vec![
            Line::styled(
                title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Line::styled(meta_str, Style::default().fg(theme.muted)),
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
    let mut state = ListState::default()
        .with_selected(Some(app.download_selected))
        .with_offset(app.download_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.download_scroll = state.offset();
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

    if app.mode == AppMode::Search {
        let cursor_offset = u16::try_from(
            app.discovery.query[..app.discovery.query_cursor]
                .chars()
                .count(),
        )
        .unwrap_or(0);
        frame.set_cursor_position((
            rows[0].x.saturating_add(3).saturating_add(cursor_offset),
            rows[0].y.saturating_add(1),
        ));
    }

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
        DiscoveryStatus::Error(_) => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("Search failed", Style::default().fg(theme.error)),
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

fn render_search_results(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    let items = app.discovery.results.iter().map(|paper| {
        let local_status = app.library.papers.iter().find(|p| {
            if let (Some(ldoi), Some(rdoi)) = (&p.doi, &paper.doi) {
                if ldoi == rdoi {
                    return true;
                }
            }
            p.title.to_lowercase() == paper.title.to_lowercase()
        }).map(|p| p.reading_status.as_str());

        let mut meta = format!(
            "{}  |  {}  |  {}",
            paper.published.format("%Y-%m-%d"),
            compact_authors(paper),
            paper
                .categories
                .first()
                .map_or("uncategorized", String::as_str)
        );
        if let Some(status) = local_status {
            meta.push_str("  |  ");
            meta.push_str(status);
        }

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
    let mut state = ListState::default()
        .with_selected(Some(app.discovery.selected))
        .with_offset(app.discovery.scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.discovery.scroll = state.offset();
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
        DiscoveryStatus::Loading => vec![ListItem::new("Loading dashboard papers...")],
        DiscoveryStatus::Error(_) => vec![ListItem::new("Dashboard papers are unavailable")],
        _ if app.today_papers.is_empty() => vec![ListItem::new("No new papers loaded")],
        _ => app
            .today_papers
            .iter()
            .take(10)
            .map(|paper| {
                let abstract_preview = compact_text(&paper.abstract_text, preview_width);
                let local_status = app.library.papers.iter().find(|p| {
                    if let (Some(ldoi), Some(rdoi)) = (&p.doi, &paper.doi) {
                        if ldoi == rdoi {
                            return true;
                        }
                    }
                    p.title.to_lowercase() == paper.title.to_lowercase()
                }).map(|p| p.reading_status.as_str());

                let mut meta_str = format!(
                    "{}  |  {}",
                    compact_authors(paper),
                    paper.published.format("%Y-%m-%d")
                );
                if let Some(status) = local_status {
                    meta_str.push_str("  |  ");
                    meta_str.push_str(status);
                }

                ListItem::new(vec![
                    Line::styled(
                        &paper.title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        meta_str,
                        Style::default().fg(theme.accent),
                    ),
                    Line::styled(abstract_preview, Style::default().fg(theme.muted)),
                    Line::raw(""),
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
                format!("Collections  {}", app.dashboard.collections),
                Style::default().fg(theme.muted),
            ),
        ]),
        storage,
    );
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
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

fn render_palette(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    let area = centered(40, 17, frame.area());
    frame.render_widget(Clear, area);

    let items = papr_core::Page::ALL.iter().map(|page| {
        ListItem::new(Line::styled(
            page.title(),
            Style::default().fg(theme.text),
        ))
    });

    let list = List::new(items)
        .block(
            Block::default()
                .title(" NAVIGATE ")
                .borders(Borders::ALL)
                .style(Style::default().bg(theme.surface))
                .border_style(Style::default().fg(theme.accent)),
        )
        .highlight_style(Style::default().bg(theme.border).fg(theme.accent))
        .highlight_symbol("> ");

    let mut state = ListState::default()
        .with_selected(Some(app.palette_selected))
        .with_offset(app.palette_scroll);
    frame.render_stateful_widget(list, area, &mut state);
    app.palette_scroll = state.offset();
}

fn render_help(frame: &mut Frame<'_>, theme: &Theme) {
    let area = centered(64, 20, frame.area());
    frame.render_widget(Clear, area);
    let help = "j / k      Move selection\nEnter/Right Open selection\nLeft       Focus navigation\nh / l      Back / open\nCtrl+p     Command palette\n/          Search arXiv\no          Open paper in webpage\np          Open local PDF\nu         Set status of a PDF as unread\nd          Download\nc          Copy citation\nn          Notes\ns          Create / Move to Collection\nB          Bookmark\nx          Delete PDF or collection\nq          Close / quit\n?          Toggle this help";
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
        ActivityItem, App, AppMode, BookmarkSummary, CollectionSummary, DiscoveryStatus,
        DownloadStatus, DownloadTask, LibraryPaper, Page, RemotePaper, Theme,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::render;

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
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        assert!(rendered_text(&terminal).contains("A Local Research Paper"));

        app.page = Page::Downloads;
        app.downloads.push(DownloadTask {
            id: "arxiv:test".into(),
            title: "A Streaming Download".into(),
            downloaded: 1024,
            total: Some(2048),
            paper_id: None,
            pdf_path: None,
            status: DownloadStatus::Running,
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
        assert!(rendered_text(&terminal).contains("A Recorded Paper"));

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
            pdf_path: Some("/tmp/paper.pdf".into()),
            file_size: Some(1024),
            reading_status: "unread".into(),
            is_favorite: false,
        });
        terminal.draw(|frame| render(frame, &mut app, &theme))?;
        let rendered = rendered_text(&terminal);
        assert!(rendered.contains("Paper In Collection"));
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
        assert!(rendered.contains("Bookmarked Research"));
        assert!(rendered.contains("Ada Lovelace, Alan Turing"));
        assert!(rendered.contains("2026"));
        assert!(rendered.contains("Terminal Studies"));
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
