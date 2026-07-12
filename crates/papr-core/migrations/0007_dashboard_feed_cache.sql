CREATE TABLE dashboard_feed_cache (
    feed_date         TEXT NOT NULL,
    keyword_signature TEXT NOT NULL,
    payload           TEXT NOT NULL,
    refreshed_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (feed_date, keyword_signature)
);
