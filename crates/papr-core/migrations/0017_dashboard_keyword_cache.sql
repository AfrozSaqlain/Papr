CREATE TABLE dashboard_keyword_cache (
    feed_date         TEXT NOT NULL,
    keyword           TEXT NOT NULL,
    payload           TEXT NOT NULL,
    refreshed_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (feed_date, keyword)
);
