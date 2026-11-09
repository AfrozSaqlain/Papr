CREATE TABLE IF NOT EXISTS pdf_highlights (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    paper_id INTEGER REFERENCES papers(id) ON DELETE CASCADE,
    pdf_path TEXT NOT NULL,
    page INTEGER NOT NULL,
    points_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_pdf_highlights_paper_page ON pdf_highlights (paper_id, page);
CREATE INDEX IF NOT EXISTS idx_pdf_highlights_path_page ON pdf_highlights (pdf_path, page);
