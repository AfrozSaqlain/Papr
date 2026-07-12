ALTER TABLE papers ADD COLUMN content_hash TEXT;
ALTER TABLE papers ADD COLUMN file_size INTEGER CHECK (file_size >= 0);
ALTER TABLE papers ADD COLUMN indexed_at TEXT;

CREATE UNIQUE INDEX idx_papers_content_hash
    ON papers(content_hash) WHERE content_hash IS NOT NULL;
CREATE INDEX idx_papers_pdf_path ON papers(pdf_path) WHERE pdf_path IS NOT NULL;

CREATE VIRTUAL TABLE paper_search USING fts5(
    title,
    abstract,
    content='papers',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER papers_search_insert AFTER INSERT ON papers BEGIN
    INSERT INTO paper_search(rowid, title, abstract)
    VALUES (new.id, new.title, COALESCE(new.abstract, ''));
END;

CREATE TRIGGER papers_search_delete AFTER DELETE ON papers BEGIN
    INSERT INTO paper_search(paper_search, rowid, title, abstract)
    VALUES ('delete', old.id, old.title, COALESCE(old.abstract, ''));
END;

CREATE TRIGGER papers_search_update AFTER UPDATE OF title, abstract ON papers BEGIN
    INSERT INTO paper_search(paper_search, rowid, title, abstract)
    VALUES ('delete', old.id, old.title, COALESCE(old.abstract, ''));
    INSERT INTO paper_search(rowid, title, abstract)
    VALUES (new.id, new.title, COALESCE(new.abstract, ''));
END;

INSERT INTO paper_search(rowid, title, abstract)
SELECT id, title, COALESCE(abstract, '') FROM papers;
