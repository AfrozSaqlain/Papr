ALTER TABLE collections ADD COLUMN folder_path TEXT;
CREATE UNIQUE INDEX idx_collections_folder_path ON collections(folder_path)
WHERE folder_path IS NOT NULL;
DELETE FROM collection_papers
WHERE rowid NOT IN (SELECT MIN(rowid) FROM collection_papers GROUP BY paper_id);
CREATE UNIQUE INDEX idx_collection_papers_exclusive ON collection_papers(paper_id);
