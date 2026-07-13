import re

with open("crates/papr/src/ui.rs", "r") as f:
    text = f.read()

# Fix the mut app in render_collection_papers call
text = text.replace(
"""fn render_collections(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    if let Some(collection) = &app.active_collection {
        render_collection_papers(frame, area, app, collection, theme);
        return;
    }""",
"""fn render_collections(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: &Theme) {
    if app.active_collection.is_some() {
        render_collection_papers(frame, area, app, theme);
        return;
    }""")

text = text.replace(
"""fn render_collection_papers(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    collection: &papr_core::CollectionSummary,
    theme: &Theme,
) {""",
"""fn render_collection_papers(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let collection = app.active_collection.as_ref().unwrap();""")

# Sidebar
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.sidebar_index\)\);\s+frame\.render_stateful_widget\(list, area, &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.sidebar_index)).with_offset(app.sidebar_scroll);\n    frame.render_stateful_widget(list, area, &mut state);\n    app.sidebar_scroll = state.offset();",
    text
)

# Collections
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.collection_selected\)\);\s+frame\.render_stateful_widget\(list, area, &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.collection_selected)).with_offset(app.collection_scroll);\n    frame.render_stateful_widget(list, area, &mut state);\n    app.collection_scroll = state.offset();",
    text
)

# Collection Papers
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.collection_paper_selected\)\);\s+frame\.render_stateful_widget\(list, rows\[1\], &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.collection_paper_selected)).with_offset(app.collection_paper_scroll);\n    frame.render_stateful_widget(list, rows[1], &mut state);\n    app.collection_paper_scroll = state.offset();",
    text
)

# Bookmarks
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.bookmark_selected\)\);\s+frame\.render_stateful_widget\(list, rows\[1\], &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.bookmark_selected)).with_offset(app.bookmark_scroll);\n    frame.render_stateful_widget(list, rows[1], &mut state);\n    app.bookmark_scroll = state.offset();",
    text
)

# Library
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.library\.selected\)\);\s+frame\.render_stateful_widget\(list, layout\[1\], &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.library.selected)).with_offset(app.library.scroll);\n    frame.render_stateful_widget(list, layout[1], &mut state);\n    app.library.scroll = state.offset();",
    text
)

# Downloads
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.download_selected\)\);\s+frame\.render_stateful_widget\(list, rows\[1\], &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.download_selected)).with_offset(app.download_scroll);\n    frame.render_stateful_widget(list, rows[1], &mut state);\n    app.download_scroll = state.offset();",
    text
)

# Discovery
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(Some\(app\.discovery\.selected\)\);\s+frame\.render_stateful_widget\(list, rows\[1\], &mut state\);",
    r"let mut state = ListState::default().with_selected(Some(app.discovery.selected)).with_offset(app.discovery.scroll);\n    frame.render_stateful_widget(list, rows[1], &mut state);\n    app.discovery.scroll = state.offset();",
    text
)

# Dashboard / Today
text = re.sub(
    r"let mut state = ListState::default\(\)\.with_selected\(selected\);\s+frame\.render_stateful_widget\(list, area, &mut state\);",
    r"let mut state = ListState::default().with_selected(selected).with_offset(app.today_scroll);\n    frame.render_stateful_widget(list, area, &mut state);\n    app.today_scroll = state.offset();",
    text
)


with open("crates/papr/src/ui.rs", "w") as f:
    f.write(text)
