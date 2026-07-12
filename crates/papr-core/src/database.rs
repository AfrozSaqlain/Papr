//! `SQLite` persistence and schema migrations.

use std::{fs, path::Path};

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::{
    library::ImportedPdf,
    models::{
        ActivityItem, BookmarkSummary, CollectionSummary, DashboardStats, LibraryPaper, PaperNote,
        ReadingDay, ReadingStatistics, RemotePaper, ResearchDashboard,
    },
};

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_library.sql")),
    (
        3,
        include_str!("../migrations/0003_research_organization.sql"),
    ),
    (4, include_str!("../migrations/0004_activity.sql")),
    (
        5,
        include_str!("../migrations/0005_merge_tags_into_collections.sql"),
    ),
];

/// Database initialization and query errors.
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// Filesystem setup failed.
    #[error("database filesystem setup failed: {0}")]
    Io(#[from] std::io::Error),
    /// `SQLite` operation failed.
    #[error("database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Local `SQLite` database with explicit migrations.
#[derive(Debug)]
pub struct Database {
    connection: Connection,
}

impl Database {
    /// Open the database, configure safe defaults, and apply pending migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory, database, or migration fails.
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    /// Create a migrated in-memory database, useful for tests and previews.
    ///
    /// # Errors
    ///
    /// Returns an error when database initialization or migration fails.
    pub fn in_memory() -> Result<Self, DatabaseError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, DatabaseError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )?;
        let mut database = Self { connection };
        database.migrate()?;
        Ok(database)
    }

    fn migrate(&mut self) -> Result<(), DatabaseError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )?;
        for &(version, sql) in MIGRATIONS {
            let applied = self
                .connection
                .query_row(
                    "SELECT version FROM schema_migrations WHERE version = ?1",
                    [version],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            if applied.is_none() {
                let transaction = self.connection.transaction()?;
                transaction.execute_batch(sql)?;
                transaction.execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    [version],
                )?;
                transaction.commit()?;
            }
        }
        Ok(())
    }

    /// Read summary counts used by the dashboard.
    ///
    /// # Errors
    ///
    /// Returns an error when the aggregate query fails.
    pub fn dashboard_stats(&self) -> Result<DashboardStats, DatabaseError> {
        let stats = self.connection.query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN reading_status = 'queued' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN pdf_path IS NOT NULL THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(is_favorite), 0)
             FROM papers",
            [],
            |row| {
                Ok(DashboardStats {
                    papers: row.get(0)?,
                    queued: row.get(1)?,
                    downloaded: row.get(2)?,
                    favorites: row.get(3)?,
                })
            },
        )?;
        Ok(stats)
    }

    /// Add a minimal paper record while enforcing DOI uniqueness.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or a database constraint fails.
    pub fn insert_paper(&self, title: &str, doi: Option<&str>) -> Result<i64, DatabaseError> {
        self.connection.execute(
            "INSERT INTO papers (title, doi) VALUES (?1, ?2)",
            params![title, doi],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Import a locally discovered PDF unless its content hash already exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be inserted or queried.
    pub fn import_pdf(&self, pdf: &ImportedPdf) -> Result<bool, DatabaseError> {
        let duplicate_id = self
            .connection
            .query_row(
                "SELECT id FROM papers WHERE content_hash = ?1 LIMIT 1",
                [&pdf.content_hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if duplicate_id.is_some() {
            return Ok(false);
        }
        let changed = self.connection.execute(
            "INSERT INTO papers (title, pdf_path, content_hash, file_size, indexed_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(pdf_path) DO UPDATE SET title = excluded.title,
                 content_hash = excluded.content_hash, file_size = excluded.file_size,
                 indexed_at = CURRENT_TIMESTAMP",
            params![
                pdf.title,
                pdf.path.to_string_lossy(),
                pdf.content_hash,
                i64::try_from(pdf.file_size).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(changed > 0)
    }

    /// List local catalog entries with author display metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the library query fails.
    pub fn library_papers(&self) -> Result<Vec<LibraryPaper>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.title,
                    COALESCE((SELECT GROUP_CONCAT(a.name, ', ')
                              FROM paper_authors pa JOIN authors a ON a.id = pa.author_id
                              WHERE pa.paper_id = p.id ORDER BY pa.position), ''),
                    p.doi, p.pdf_path, p.file_size, p.reading_status, p.is_favorite
             FROM papers p ORDER BY p.created_at DESC, p.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let file_size: Option<i64> = row.get(5)?;
            Ok(LibraryPaper {
                id: row.get(0)?,
                title: row.get(1)?,
                authors: row.get(2)?,
                doi: row.get(3)?,
                pdf_path: row.get(4)?,
                file_size: file_size.and_then(|size| u64::try_from(size).ok()),
                reading_status: row.get(6)?,
                is_favorite: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Upsert downloaded arXiv metadata and attach its local PDF path.
    ///
    /// # Errors
    ///
    /// Returns an error when paper or author persistence fails.
    pub fn attach_download(
        &mut self,
        paper: &RemotePaper,
        pdf: &ImportedPdf,
    ) -> Result<i64, DatabaseError> {
        let transaction = self.connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT id FROM papers
                 WHERE arxiv_id = ?1 OR content_hash = ?2 OR (?3 IS NOT NULL AND doi = ?3)
                    OR pdf_path = ?4
                 ORDER BY CASE WHEN arxiv_id = ?1 THEN 0 ELSE 1 END LIMIT 1",
                params![
                    paper.id,
                    pdf.content_hash,
                    paper.doi,
                    pdf.path.to_string_lossy()
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let file_size = i64::try_from(pdf.file_size).unwrap_or(i64::MAX);
        let paper_id = if let Some(paper_id) = existing {
            transaction.execute(
                "UPDATE papers SET title = ?1, abstract = ?2, doi = ?3, arxiv_id = ?4,
                    published_at = ?5, updated_at = ?6, pdf_path = ?7, content_hash = ?8,
                    file_size = ?9, indexed_at = CURRENT_TIMESTAMP WHERE id = ?10",
                params![
                    paper.title,
                    paper.abstract_text,
                    paper.doi,
                    paper.id,
                    paper.published.to_rfc3339(),
                    paper.updated.to_rfc3339(),
                    pdf.path.to_string_lossy(),
                    pdf.content_hash,
                    file_size,
                    paper_id
                ],
            )?;
            paper_id
        } else {
            transaction.execute(
                "INSERT INTO papers
                    (title, abstract, doi, arxiv_id, published_at, updated_at, pdf_path,
                     content_hash, file_size, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP)",
                params![
                    paper.title,
                    paper.abstract_text,
                    paper.doi,
                    paper.id,
                    paper.published.to_rfc3339(),
                    paper.updated.to_rfc3339(),
                    pdf.path.to_string_lossy(),
                    pdf.content_hash,
                    file_size
                ],
            )?;
            transaction.last_insert_rowid()
        };
        transaction.execute("DELETE FROM paper_authors WHERE paper_id = ?1", [paper_id])?;
        for (position, author) in paper.authors.iter().enumerate() {
            let author_id = if let Some(id) = transaction
                .query_row(
                    "SELECT id FROM authors WHERE name = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
                    [author],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
            {
                id
            } else {
                transaction.execute("INSERT INTO authors (name) VALUES (?1)", [author])?;
                transaction.last_insert_rowid()
            };
            transaction.execute(
                "INSERT INTO paper_authors (paper_id, author_id, position) VALUES (?1, ?2, ?3)",
                params![
                    paper_id,
                    author_id,
                    i64::try_from(position).unwrap_or(i64::MAX)
                ],
            )?;
        }
        transaction.commit()?;
        Ok(paper_id)
    }

    /// Ensure a remote paper has a local metadata record and return its identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata cannot be queried or persisted.
    pub fn ensure_remote_paper(&self, paper: &RemotePaper) -> Result<i64, DatabaseError> {
        let existing = self
            .connection
            .query_row(
                "SELECT id FROM papers WHERE arxiv_id = ?1 OR (?2 IS NOT NULL AND doi = ?2)
                 ORDER BY CASE WHEN arxiv_id = ?1 THEN 0 ELSE 1 END LIMIT 1",
                params![paper.id, paper.doi],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.connection.execute(
                "UPDATE papers SET title = ?1, abstract = ?2, arxiv_id = ?3,
                 doi = COALESCE(?4, doi), published_at = ?5, updated_at = ?6 WHERE id = ?7",
                params![
                    paper.title,
                    paper.abstract_text,
                    paper.id,
                    paper.doi,
                    paper.published.to_rfc3339(),
                    paper.updated.to_rfc3339(),
                    id
                ],
            )?;
            return Ok(id);
        }
        self.connection.execute(
            "INSERT INTO papers (title, abstract, arxiv_id, doi, published_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                paper.title,
                paper.abstract_text,
                paper.id,
                paper.doi,
                paper.published.to_rfc3339(),
                paper.updated.to_rfc3339()
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Load the note for a paper, returning an empty draft when absent.
    ///
    /// # Errors
    ///
    /// Returns an error when the note query fails.
    pub fn paper_note(&self, paper_id: i64) -> Result<PaperNote, DatabaseError> {
        Ok(self
            .connection
            .query_row(
                "SELECT title, body FROM notes WHERE paper_id = ?1",
                [paper_id],
                |row| {
                    Ok(PaperNote {
                        paper_id,
                        title: row.get(0)?,
                        body: row.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or(PaperNote {
                paper_id,
                title: String::new(),
                body: String::new(),
            }))
    }

    /// Insert or update a paper's Markdown note.
    ///
    /// # Errors
    ///
    /// Returns an error when the note cannot be persisted.
    pub fn save_note(&self, note: &PaperNote) -> Result<(), DatabaseError> {
        self.connection.execute(
            "INSERT INTO notes (paper_id, title, body) VALUES (?1, ?2, ?3)
             ON CONFLICT(paper_id) DO UPDATE SET title = excluded.title,
                 body = excluded.body, updated_at = CURRENT_TIMESTAMP",
            params![note.paper_id, note.title, note.body],
        )?;
        Ok(())
    }

    /// Add a paper to a named collection, creating it when necessary.
    ///
    /// # Errors
    ///
    /// Returns an error when the collection assignment cannot be persisted.
    pub fn add_to_collection(&self, paper_id: i64, name: &str) -> Result<(), DatabaseError> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        self.connection.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?1)",
            [name],
        )?;
        let collection_id = self.connection.query_row(
            "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
            [name],
            |row| row.get::<_, i64>(0),
        )?;
        self.connection.execute(
            "INSERT OR IGNORE INTO collection_papers (collection_id, paper_id) VALUES (?1, ?2)",
            params![collection_id, paper_id],
        )?;
        Ok(())
    }

    /// Toggle a whole-paper bookmark and return whether it is now active.
    ///
    /// # Errors
    ///
    /// Returns an error when the bookmark cannot be queried or changed.
    pub fn toggle_bookmark(&self, paper_id: i64) -> Result<bool, DatabaseError> {
        let existing = self.connection.query_row(
            "SELECT id FROM bookmarks WHERE paper_id = ?1 AND page IS NULL AND note_offset IS NULL",
            [paper_id], |row| row.get::<_, i64>(0),
        ).optional()?;
        if let Some(id) = existing {
            self.connection
                .execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
            Ok(false)
        } else {
            self.connection
                .execute("INSERT INTO bookmarks (paper_id) VALUES (?1)", [paper_id])?;
            Ok(true)
        }
    }

    /// List collections with paper counts.
    ///
    /// # Errors
    ///
    /// Returns an error when the collection query fails.
    pub fn collections(&self) -> Result<Vec<CollectionSummary>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.name, COUNT(cp.paper_id) FROM collections c
             LEFT JOIN collection_papers cp ON cp.collection_id = c.id
             GROUP BY c.id ORDER BY c.name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(CollectionSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                paper_count: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// List papers assigned to a collection in newest-first library order.
    ///
    /// # Errors
    ///
    /// Returns an error when the collection-paper query fails.
    pub fn papers_for_collection(
        &self,
        collection_id: i64,
    ) -> Result<Vec<LibraryPaper>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.title,
                    COALESCE((SELECT GROUP_CONCAT(a.name, ', ')
                              FROM paper_authors pa JOIN authors a ON a.id = pa.author_id
                              WHERE pa.paper_id = p.id ORDER BY pa.position), ''),
                    p.doi, p.pdf_path, p.file_size, p.reading_status, p.is_favorite
             FROM collection_papers cp JOIN papers p ON p.id = cp.paper_id
             WHERE cp.collection_id = ?1 ORDER BY p.created_at DESC, p.id DESC",
        )?;
        let rows = statement.query_map([collection_id], |row| {
            let file_size: Option<i64> = row.get(5)?;
            Ok(LibraryPaper {
                id: row.get(0)?,
                title: row.get(1)?,
                authors: row.get(2)?,
                doi: row.get(3)?,
                pdf_path: row.get(4)?,
                file_size: file_size.and_then(|size| u64::try_from(size).ok()),
                reading_status: row.get(6)?,
                is_favorite: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// List bookmarks newest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the bookmark query fails.
    pub fn bookmarks(&self) -> Result<Vec<BookmarkSummary>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT b.id, b.paper_id, p.title, b.page, b.label FROM bookmarks b
             JOIN papers p ON p.id = b.paper_id ORDER BY b.created_at DESC, b.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let page: Option<i64> = row.get(3)?;
            Ok(BookmarkSummary {
                id: row.get(0)?,
                paper_id: row.get(1)?,
                paper_title: row.get(2)?,
                page: page.and_then(|value| u32::try_from(value).ok()),
                label: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Record a paper or PDF open in reading history and recent activity.
    ///
    /// # Errors
    ///
    /// Returns an error when either history record cannot be inserted.
    pub fn record_open(&self, paper_id: i64, pdf: bool) -> Result<(), DatabaseError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO reading_history (paper_id) VALUES (?1)",
            [paper_id],
        )?;
        transaction.execute(
            "INSERT INTO activity_log (kind, paper_id) VALUES (?1, ?2)",
            params![if pdf { "pdf_opened" } else { "paper_opened" }, paper_id],
        )?;
        transaction.execute(
            "UPDATE papers SET last_opened_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [paper_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Record a non-reading research activity.
    ///
    /// # Errors
    ///
    /// Returns an error when the event cannot be inserted.
    pub fn record_activity(
        &self,
        kind: &str,
        paper_id: Option<i64>,
        detail: Option<&str>,
    ) -> Result<(), DatabaseError> {
        self.connection.execute(
            "INSERT INTO activity_log (kind, paper_id, detail) VALUES (?1, ?2, ?3)",
            params![kind, paper_id, detail],
        )?;
        Ok(())
    }

    /// Read the full dashboard snapshot in a bounded set of local queries.
    ///
    /// # Errors
    ///
    /// Returns an error when any aggregate or activity query fails.
    pub fn research_dashboard(&self) -> Result<ResearchDashboard, DatabaseError> {
        let counts = self.dashboard_stats()?;
        let (unread, collections, disk_usage) = self.connection.query_row(
            "SELECT
                (SELECT COUNT(*) FROM papers WHERE reading_status != 'read'),
                (SELECT COUNT(*) FROM collections),
                (SELECT COALESCE(SUM(file_size), 0) FROM papers)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let page_count: u64 = self
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: u64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        Ok(ResearchDashboard {
            counts,
            unread,
            collections,
            disk_usage,
            database_size: page_count.saturating_mul(page_size),
            reading: self.reading_statistics()?,
            recent_activity: self.recent_activity(12)?,
        })
    }

    /// Compute persisted reading metrics and an 84-day activity heatmap.
    ///
    /// # Errors
    ///
    /// Returns an error when history aggregates cannot be queried.
    pub fn reading_statistics(&self) -> Result<ReadingStatistics, DatabaseError> {
        let now = Utc::now();
        let (sessions, monthly_reading, yearly_reading, average_reading_seconds) =
            self.connection.query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN strftime('%Y-%m', opened_at) = ?1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN strftime('%Y', opened_at) = ?2 THEN 1 ELSE 0 END), 0),
                    CAST(COALESCE(AVG(duration_s), 0) AS INTEGER)
                 FROM reading_history",
                params![now.format("%Y-%m").to_string(), now.year().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let heatmap = self.reading_heatmap()?;
        let dates = heatmap
            .iter()
            .filter(|day| day.count > 0)
            .filter_map(|day| NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").ok())
            .collect::<std::collections::HashSet<_>>();
        let current_streak = reading_streak(&dates, now.date_naive());
        let most_active_day = self
            .connection
            .query_row(
                "SELECT strftime('%w', opened_at), COUNT(*) FROM reading_history
                 GROUP BY strftime('%w', opened_at) ORDER BY COUNT(*) DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|day| weekday_name(&day).map(str::to_owned));
        let most_read_author = self.top_history_label(
            "SELECT a.name FROM reading_history h
             JOIN paper_authors pa ON pa.paper_id = h.paper_id
             JOIN authors a ON a.id = pa.author_id
             GROUP BY a.id ORDER BY COUNT(*) DESC, a.name LIMIT 1",
        )?;
        let most_read_journal = self.top_history_label(
            "SELECT p.journal FROM reading_history h JOIN papers p ON p.id = h.paper_id
             WHERE p.journal IS NOT NULL AND trim(p.journal) != ''
             GROUP BY p.journal ORDER BY COUNT(*) DESC, p.journal LIMIT 1",
        )?;
        Ok(ReadingStatistics {
            current_streak,
            monthly_reading,
            yearly_reading,
            sessions,
            average_reading_seconds,
            most_active_day,
            most_read_author,
            most_read_journal,
            heatmap,
        })
    }

    fn reading_heatmap(&self) -> Result<Vec<ReadingDay>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT date(opened_at), COUNT(*) FROM reading_history
             WHERE opened_at >= datetime('now', '-83 days')
             GROUP BY date(opened_at) ORDER BY date(opened_at)",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        let counts = rows.collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        let today = Utc::now().date_naive();
        Ok((0_i64..84)
            .rev()
            .map(|offset| {
                let date = today - Duration::days(offset);
                let date = date.format("%Y-%m-%d").to_string();
                ReadingDay {
                    count: counts.get(&date).copied().unwrap_or(0),
                    date,
                }
            })
            .collect())
    }

    fn top_history_label(&self, sql: &str) -> Result<Option<String>, DatabaseError> {
        self.connection
            .query_row(sql, [], |row| row.get(0))
            .optional()
            .map_err(Into::into)
    }

    /// Return recent activity for dashboard and history views.
    ///
    /// # Errors
    ///
    /// Returns an error when the activity query fails.
    pub fn recent_activity(&self, limit: u16) -> Result<Vec<ActivityItem>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "SELECT a.kind, COALESCE(p.title, a.detail, a.kind), a.occurred_at
             FROM activity_log a LEFT JOIN papers p ON p.id = a.paper_id
             ORDER BY a.occurred_at DESC, a.id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            let occurred_at: NaiveDateTime = row.get(2)?;
            Ok(ActivityItem {
                kind: row.get(0)?,
                label: row.get(1)?,
                occurred_at: occurred_at.and_utc(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn reading_streak(dates: &std::collections::HashSet<NaiveDate>, today: NaiveDate) -> u64 {
    let mut cursor = if dates.contains(&today) {
        today
    } else {
        today - Duration::days(1)
    };
    let mut streak = 0_u64;
    while dates.contains(&cursor) {
        streak = streak.saturating_add(1);
        cursor -= Duration::days(1);
    }
    streak
}

fn weekday_name(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("Sunday"),
        "1" => Some("Monday"),
        "2" => Some("Tuesday"),
        "3" => Some("Wednesday"),
        "4" => Some("Thursday"),
        "5" => Some("Friday"),
        "6" => Some("Saturday"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use rusqlite::params;

    use super::Database;
    use crate::{
        library::ImportedPdf,
        models::{PaperNote, RemotePaper},
    };

    #[test]
    fn migrations_create_queryable_schema() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        database.insert_paper("A useful paper", Some("10.1000/test"))?;
        let stats = database.dashboard_stats()?;
        assert_eq!(stats.papers, 1);
        Ok(())
    }

    #[test]
    fn doi_is_unique_when_present() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        database.insert_paper("First", Some("10.1000/same"))?;
        assert!(
            database
                .insert_paper("Second", Some("10.1000/same"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn content_hash_prevents_duplicate_imports() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let first = imported_pdf("first.pdf", "same-hash");
        let duplicate = imported_pdf("copy.pdf", "same-hash");
        assert!(database.import_pdf(&first)?);
        assert!(!database.import_pdf(&duplicate)?);
        assert_eq!(database.library_papers()?.len(), 1);
        Ok(())
    }

    #[test]
    fn changed_file_at_same_path_refreshes_index() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let original = imported_pdf("paper.pdf", "old-hash");
        let mut replacement = imported_pdf("paper.pdf", "new-hash");
        replacement.file_size = 84;
        database.import_pdf(&original)?;
        assert!(database.import_pdf(&replacement)?);
        let rows = database.library_papers()?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_size, Some(84));
        Ok(())
    }

    #[test]
    fn notes_collections_and_bookmarks_are_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let paper_id = database.insert_paper("Organized paper", None)?;
        let note = PaperNote {
            paper_id,
            title: "Reading notes".into(),
            body: "# First pass".into(),
        };
        database.save_note(&note)?;
        let mut revised = note;
        revised.body = "# Revised".into();
        database.save_note(&revised)?;
        assert_eq!(database.paper_note(paper_id)?.body, "# Revised");

        database.add_to_collection(paper_id, "Journal Club")?;
        database.add_to_collection(paper_id, "journal club")?;
        assert_eq!(database.collections()?.len(), 1);
        assert_eq!(database.collections()?[0].paper_count, 1);
        let collection_id = database.collections()?[0].id;
        let collection_papers = database.papers_for_collection(collection_id)?;
        assert_eq!(collection_papers.len(), 1);
        assert_eq!(collection_papers[0].title, "Organized paper");

        assert!(database.toggle_bookmark(paper_id)?);
        assert_eq!(database.bookmarks()?.len(), 1);
        assert!(!database.toggle_bookmark(paper_id)?);
        assert!(database.bookmarks()?.is_empty());
        Ok(())
    }

    #[test]
    fn legacy_tags_are_merged_into_selectable_collections() -> Result<(), Box<dyn std::error::Error>>
    {
        let database = Database::in_memory()?;
        let paper_id = database.insert_paper("Legacy tagged paper", None)?;
        database
            .connection
            .execute("INSERT INTO tags (name) VALUES ('review')", [])?;
        let tag_id = database.connection.last_insert_rowid();
        database.connection.execute(
            "INSERT INTO paper_tags (paper_id, tag_id) VALUES (?1, ?2)",
            params![paper_id, tag_id],
        )?;
        database.connection.execute_batch(include_str!(
            "../migrations/0005_merge_tags_into_collections.sql"
        ))?;

        let collections = database.collections()?;
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "review");
        assert_eq!(collections[0].paper_count, 1);
        let papers = database.papers_for_collection(collections[0].id)?;
        assert_eq!(papers[0].title, "Legacy tagged paper");
        Ok(())
    }

    #[test]
    fn download_enriches_matching_local_pdf() -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::in_memory()?;
        let pdf = imported_pdf("paper.pdf", "download-hash");
        database.import_pdf(&pdf)?;
        let timestamp = Utc
            .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
            .single()
            .ok_or("invalid test timestamp")?;
        let paper = RemotePaper {
            id: "https://arxiv.org/abs/2602.00001".into(),
            title: "Canonical title".into(),
            authors: vec!["Researcher One".into()],
            abstract_text: "Indexed abstract".into(),
            published: timestamp,
            updated: timestamp,
            categories: vec!["cs.DL".into()],
            pdf_url: Some("https://arxiv.org/pdf/2602.00001".into()),
            doi: Some("10.1000/download".into()),
            journal_ref: None,
        };
        database.attach_download(&paper, &pdf)?;
        let rows = database.library_papers()?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Canonical title");
        assert_eq!(rows[0].authors, "Researcher One");
        Ok(())
    }

    #[test]
    fn recorded_opens_feed_dashboard_history_and_statistics()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let paper_id = database.insert_paper("History paper", None)?;
        database.record_open(paper_id, true)?;
        database.record_activity("search", None, Some("gravity waves"))?;

        let dashboard = database.research_dashboard()?;
        assert_eq!(dashboard.reading.sessions, 1);
        assert_eq!(dashboard.reading.current_streak, 1);
        assert_eq!(dashboard.reading.heatmap.len(), 84);
        assert_eq!(dashboard.recent_activity.len(), 2);
        assert_eq!(dashboard.recent_activity[0].label, "gravity waves");
        assert_eq!(dashboard.recent_activity[1].label, "History paper");
        Ok(())
    }

    fn imported_pdf(path: &str, hash: &str) -> ImportedPdf {
        ImportedPdf {
            path: PathBuf::from(path),
            title: "Imported title".into(),
            content_hash: hash.into(),
            file_size: 42,
        }
    }
}
