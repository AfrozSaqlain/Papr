CREATE TABLE papers (
    id              INTEGER PRIMARY KEY,
    title           TEXT NOT NULL CHECK (length(trim(title)) > 0),
    abstract        TEXT,
    doi             TEXT COLLATE NOCASE,
    arxiv_id        TEXT COLLATE NOCASE,
    journal         TEXT,
    published_at    TEXT,
    updated_at      TEXT,
    pdf_path        TEXT,
    reading_status  TEXT NOT NULL DEFAULT 'unread'
                    CHECK (reading_status IN ('unread', 'queued', 'reading', 'read')),
    is_favorite     INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
    rating          INTEGER CHECK (rating BETWEEN 1 AND 5),
    created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_opened_at  TEXT,
    UNIQUE (doi),
    UNIQUE (arxiv_id),
    UNIQUE (pdf_path)
);

CREATE TABLE authors (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    openalex_id TEXT COLLATE NOCASE UNIQUE,
    orcid       TEXT COLLATE NOCASE UNIQUE
);

CREATE TABLE paper_authors (
    paper_id    INTEGER NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    author_id   INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (paper_id, author_id)
);

CREATE TABLE collections (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL COLLATE NOCASE UNIQUE,
    description TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE collection_papers (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    paper_id      INTEGER NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    added_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (collection_id, paper_id)
);

CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE
);

CREATE TABLE paper_tags (
    paper_id INTEGER NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (paper_id, tag_id)
);

CREATE TABLE notes (
    id         INTEGER PRIMARY KEY,
    paper_id   INTEGER REFERENCES papers(id) ON DELETE CASCADE,
    title      TEXT NOT NULL DEFAULT '',
    body       TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE reading_history (
    id         INTEGER PRIMARY KEY,
    paper_id   INTEGER NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    opened_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    page       INTEGER,
    duration_s INTEGER NOT NULL DEFAULT 0 CHECK (duration_s >= 0)
);

CREATE INDEX idx_papers_title ON papers(title COLLATE NOCASE);
CREATE INDEX idx_papers_status ON papers(reading_status);
CREATE INDEX idx_papers_created ON papers(created_at DESC);
CREATE INDEX idx_authors_name ON authors(name COLLATE NOCASE);
CREATE INDEX idx_history_opened ON reading_history(opened_at DESC);
CREATE INDEX idx_notes_paper ON notes(paper_id);

