ALTER TABLE papers ADD COLUMN enrichment_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE papers ADD COLUMN enrichment_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE papers ADD COLUMN last_enrichment_attempt TEXT;

UPDATE papers SET enrichment_status = 'success'
WHERE EXISTS (SELECT 1 FROM paper_authors WHERE paper_id = papers.id);
