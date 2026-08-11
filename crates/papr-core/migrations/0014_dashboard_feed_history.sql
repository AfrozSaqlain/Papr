CREATE TABLE dashboard_feed_history (
    feed_date         TEXT NOT NULL,
    keyword_signature TEXT NOT NULL,
    paper_id          TEXT NOT NULL,
    displayed_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (feed_date, paper_id)
);

CREATE INDEX idx_dashboard_feed_history_feed_date
    ON dashboard_feed_history(feed_date);
