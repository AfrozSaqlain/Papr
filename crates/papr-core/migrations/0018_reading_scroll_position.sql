ALTER TABLE papers ADD COLUMN last_read_scroll_ratio REAL NOT NULL DEFAULT 0.0 CHECK (last_read_scroll_ratio >= 0.0);
