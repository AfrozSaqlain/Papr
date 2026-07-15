CREATE TABLE reading_queue (
    paper_id INTEGER PRIMARY KEY REFERENCES papers(id) ON DELETE CASCADE,
    queue_order INTEGER NOT NULL UNIQUE
);

CREATE TRIGGER remove_from_queue_on_status_change
AFTER UPDATE OF reading_status ON papers
FOR EACH ROW
WHEN new.reading_status != 'queued'
BEGIN
    DELETE FROM reading_queue WHERE paper_id = old.id;
END;
