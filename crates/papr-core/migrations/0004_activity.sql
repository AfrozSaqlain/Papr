CREATE TABLE activity_log (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN (
                    'paper_opened', 'pdf_opened', 'note_opened', 'search',
                    'downloaded', 'bookmarked', 'tagged', 'collected'
                )),
    paper_id    INTEGER REFERENCES papers(id) ON DELETE CASCADE,
    detail      TEXT,
    occurred_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_activity_occurred ON activity_log(occurred_at DESC);
CREATE INDEX idx_activity_paper ON activity_log(paper_id, occurred_at DESC);

