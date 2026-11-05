ALTER TABLE papers ADD COLUMN last_read_page INTEGER NOT NULL DEFAULT 1 CHECK (last_read_page >= 1);
CREATE INDEX IF NOT EXISTS idx_papers_last_opened ON papers(last_opened_at);
