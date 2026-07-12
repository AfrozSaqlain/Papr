INSERT OR IGNORE INTO collections (name, description)
SELECT name, 'Migrated from legacy tag'
FROM tags;

INSERT OR IGNORE INTO collection_papers (collection_id, paper_id)
SELECT c.id, pt.paper_id
FROM paper_tags pt
JOIN tags t ON t.id = pt.tag_id
JOIN collections c ON c.name = t.name COLLATE NOCASE;
