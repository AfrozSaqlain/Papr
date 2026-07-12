CREATE UNIQUE INDEX idx_notes_one_per_paper ON notes(paper_id);

CREATE TABLE bookmarks (
    id          INTEGER PRIMARY KEY,
    paper_id    INTEGER NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    page        INTEGER,
    note_offset INTEGER,
    label       TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (paper_id, page, note_offset)
);

CREATE INDEX idx_bookmarks_paper ON bookmarks(paper_id);
CREATE UNIQUE INDEX idx_bookmarks_position ON bookmarks(
    paper_id,
    COALESCE(page, -1),
    COALESCE(note_offset, -1)
);
CREATE INDEX idx_collection_papers_paper ON collection_papers(paper_id);
CREATE INDEX idx_paper_tags_tag ON paper_tags(tag_id);
