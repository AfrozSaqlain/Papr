PRAGMA foreign_keys=OFF;

CREATE TABLE activity_log_new (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (
                    kind IN (
                        'paper_opened', 'pdf_opened', 'note_opened', 'search',
                        'downloaded', 'bookmarked', 'tagged', 'collected', 'paper_browsed',
                        'project_opened', 'project_created', 'project_renamed', 'project_deleted'
                    )
                ),
    paper_id    INTEGER REFERENCES papers(id) ON DELETE CASCADE,
    detail      TEXT,
    occurred_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO activity_log_new (id, kind, paper_id, detail, occurred_at)
SELECT id, kind, paper_id, detail, occurred_at FROM activity_log;

DROP TABLE activity_log;

ALTER TABLE activity_log_new RENAME TO activity_log;

CREATE INDEX idx_activity_occurred ON activity_log(occurred_at DESC);
CREATE INDEX idx_activity_paper ON activity_log(paper_id, occurred_at DESC);

PRAGMA foreign_keys=ON;
