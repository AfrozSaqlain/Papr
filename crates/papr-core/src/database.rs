//! `SQLite` persistence and schema migrations.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::{
    library::{CollectionDirectory, ImportedPdf},
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
    (
        6,
        include_str!("../migrations/0006_filesystem_collections.sql"),
    ),
    (
        7,
        include_str!("../migrations/0007_dashboard_feed_cache.sql"),
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
    /// Cached JSON payload could not be encoded or decoded.
    #[error("database JSON payload failed: {0}")]
    Json(#[from] serde_json::Error),
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

    /// Resolve an imported PDF by content hash or current path.
    ///
    /// # Errors
    /// Returns an error when the lookup fails.
    pub fn paper_id_for_pdf(&self, pdf: &ImportedPdf) -> Result<Option<i64>, DatabaseError> {
        self.connection
            .query_row(
                "SELECT id FROM papers WHERE content_hash = ?1 OR pdf_path = ?2 LIMIT 1",
                params![pdf.content_hash, pdf.path.to_string_lossy()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// List catalog entries that have a local PDF with author display metadata.
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
             FROM papers p
             WHERE p.pdf_path IS NOT NULL
             ORDER BY p.created_at DESC, p.id DESC",
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

    /// List local PDF-backed papers whose files are inside the configured roots.
    ///
    /// # Errors
    ///
    /// Returns an error when the library query fails.
    pub fn library_papers_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<LibraryPaper>, DatabaseError> {
        Ok(self
            .library_papers()?
            .into_iter()
            .filter(|paper| {
                paper.pdf_path.as_deref().is_some_and(|path| {
                    let path = Path::new(path);
                    path.is_file() && roots.iter().any(|root| path.starts_with(root))
                })
            })
            .collect())
    }

    /// Load the cached dashboard feed for one local date and keyword set.
    ///
    /// # Errors
    ///
    /// Returns an error when the query or JSON decoding fails.
    pub fn dashboard_feed_cache(
        &self,
        feed_date: &str,
        keyword_signature: &str,
    ) -> Result<Option<Vec<RemotePaper>>, DatabaseError> {
        self.connection
            .query_row(
                "SELECT payload FROM dashboard_feed_cache
                 WHERE feed_date = ?1 AND keyword_signature = ?2",
                params![feed_date, keyword_signature],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|payload| serde_json::from_str(&payload).map_err(Into::into))
            .transpose()
    }

    /// Save the dashboard feed for one local date and keyword set.
    ///
    /// # Errors
    ///
    /// Returns an error when JSON encoding or persistence fails.
    pub fn save_dashboard_feed_cache(
        &self,
        feed_date: &str,
        keyword_signature: &str,
        papers: &[RemotePaper],
    ) -> Result<(), DatabaseError> {
        let payload = serde_json::to_string(papers)?;
        self.connection.execute(
            "INSERT INTO dashboard_feed_cache
                (feed_date, keyword_signature, payload, refreshed_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
             ON CONFLICT(feed_date, keyword_signature) DO UPDATE SET
                payload = excluded.payload,
                refreshed_at = CURRENT_TIMESTAMP",
            params![feed_date, keyword_signature, payload],
        )?;
        Ok(())
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
            "SELECT c.id, c.name, COUNT(cp.paper_id), c.folder_path FROM collections c
             LEFT JOIN collection_papers cp ON cp.collection_id = c.id
             GROUP BY c.id ORDER BY c.name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(CollectionSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                paper_count: row.get(2)?,
                folder_path: row.get(3)?,
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

    /// Synchronize exclusive collection membership from a PDF's directory.
    ///
    /// # Errors
    /// Returns an error when collection persistence fails.
    pub fn sync_pdf_collection(
        &self,
        paper_id: i64,
        pdf: &ImportedPdf,
    ) -> Result<(), DatabaseError> {
        let Some(relative) = &pdf.relative_directory else {
            self.connection.execute(
                "DELETE FROM collection_papers WHERE paper_id = ?1",
                [paper_id],
            )?;
            return Ok(());
        };
        let Some(root) = &pdf.library_root else {
            return Ok(());
        };
        let folder = root.join(relative).to_string_lossy().into_owned();
        let base_name = relative.to_string_lossy().replace('\\', "/");
        let tx = self.connection.unchecked_transaction()?;
        let mut collection_id = tx
            .query_row(
                "SELECT id FROM collections WHERE folder_path = ?1",
                [&folder],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if collection_id.is_none() {
            let logical_collection = tx
                .query_row(
                    "SELECT id, folder_path FROM collections WHERE name = ?1 COLLATE NOCASE",
                    [&base_name],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            if let Some((id, None)) = logical_collection {
                tx.execute(
                    "UPDATE collections SET folder_path = ?1 WHERE id = ?2",
                    params![folder, id],
                )?;
                collection_id = Some(id);
            } else {
                let root_label = root
                    .file_name()
                    .and_then(|part| part.to_str())
                    .unwrap_or("library");
                let mut candidate = base_name.clone();
                let mut suffix = 1_u32;
                while tx
                    .query_row(
                        "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
                        [&candidate],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .is_some()
                {
                    candidate = if suffix == 1 {
                        format!("{base_name} ({root_label})")
                    } else {
                        format!("{base_name} ({root_label} {suffix})")
                    };
                    suffix += 1;
                }
                tx.execute(
                    "INSERT INTO collections (name, folder_path) VALUES (?1, ?2)",
                    params![candidate, folder],
                )?;
                collection_id = Some(tx.last_insert_rowid());
            }
        }
        let collection_id = collection_id.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.execute(
            "DELETE FROM collection_papers WHERE paper_id = ?1",
            [paper_id],
        )?;
        tx.execute(
            "INSERT INTO collection_papers (collection_id, paper_id) VALUES (?1, ?2)",
            params![collection_id, paper_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Ensure a discovered library subdirectory has a filesystem-backed collection.
    ///
    /// # Errors
    /// Returns an error when collection persistence fails.
    pub fn sync_collection_directory(
        &self,
        directory: &CollectionDirectory,
    ) -> Result<(), DatabaseError> {
        let folder = directory
            .library_root
            .join(&directory.relative_path)
            .to_string_lossy()
            .into_owned();
        if self
            .connection
            .query_row(
                "SELECT id FROM collections WHERE folder_path = ?1",
                [&folder],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Ok(());
        }
        let base_name = directory.relative_path.to_string_lossy().replace('\\', "/");
        if let Some(id) = self
            .connection
            .query_row(
                "SELECT id FROM collections
                 WHERE name = ?1 COLLATE NOCASE AND folder_path IS NULL",
                [&base_name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            self.connection.execute(
                "UPDATE collections SET folder_path = ?1 WHERE id = ?2",
                params![folder, id],
            )?;
            return Ok(());
        }
        let root_label = directory
            .library_root
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("library");
        let mut candidate = base_name.clone();
        let mut suffix = 1_u32;
        while self
            .connection
            .query_row(
                "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
                [&candidate],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            candidate = if suffix == 1 {
                format!("{base_name} ({root_label})")
            } else {
                format!("{base_name} ({root_label} {suffix})")
            };
            suffix += 1;
        }
        self.connection.execute(
            "INSERT INTO collections (name, folder_path) VALUES (?1, ?2)",
            params![candidate, folder],
        )?;
        Ok(())
    }

    /// Update a paper path and assign it exclusively to a collection.
    ///
    /// # Errors
    /// Returns an error when the transaction fails.
    pub fn assign_moved_pdf(
        &self,
        paper_id: i64,
        collection_id: i64,
        new_path: &Path,
    ) -> Result<(), DatabaseError> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "UPDATE papers SET pdf_path = ?1 WHERE id = ?2",
            params![new_path.to_string_lossy(), paper_id],
        )?;
        tx.execute(
            "DELETE FROM collection_papers WHERE paper_id = ?1",
            [paper_id],
        )?;
        tx.execute(
            "INSERT INTO collection_papers (collection_id, paper_id) VALUES (?1, ?2)",
            params![collection_id, paper_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Create a filesystem-backed collection.
    ///
    /// # Errors
    /// Returns an error when the collection cannot be inserted.
    pub fn create_collection(&self, name: &str, folder: &Path) -> Result<i64, DatabaseError> {
        self.connection.execute(
            "INSERT INTO collections (name, folder_path) VALUES (?1, ?2)",
            params![name, folder.to_string_lossy()],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Attach a backing folder to an existing logical collection.
    ///
    /// # Errors
    /// Returns an error when the collection cannot be updated.
    pub fn set_collection_folder(&self, id: i64, folder: &Path) -> Result<(), DatabaseError> {
        self.connection.execute(
            "UPDATE collections SET folder_path = ?1 WHERE id = ?2",
            params![folder.to_string_lossy(), id],
        )?;
        Ok(())
    }

    /// Rename a filesystem-backed collection and rewrite stored PDF paths.
    ///
    /// # Errors
    /// Returns an error when the transaction fails.
    pub fn rename_collection(
        &self,
        id: i64,
        name: &str,
        old: &Path,
        new: &Path,
    ) -> Result<(), DatabaseError> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "UPDATE collections SET name = ?1, folder_path = ?2 WHERE id = ?3",
            params![name, new.to_string_lossy(), id],
        )?;
        let old = old.to_string_lossy();
        let new = new.to_string_lossy();
        tx.execute(
            "UPDATE papers SET pdf_path = ?1 || substr(pdf_path, length(?2) + 1)
             WHERE id IN (SELECT paper_id FROM collection_papers WHERE collection_id = ?3)",
            params![new, old, id],
        )?;
        tx.commit()?;
        Ok(())
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
    use std::{fs, path::PathBuf};

    use chrono::{TimeZone, Utc};
    use rusqlite::params;

    use super::Database;
    use crate::{
        library::{CollectionDirectory, ImportedPdf},
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
    fn remote_only_papers_do_not_appear_in_library() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let timestamp = Utc
            .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
            .single()
            .ok_or("invalid test timestamp")?;
        let paper = RemotePaper {
            id: "https://arxiv.org/abs/2602.00002".into(),
            title: "Search result only".into(),
            authors: vec!["Remote Author".into()],
            abstract_text: "Metadata without a downloaded PDF.".into(),
            published: timestamp,
            updated: timestamp,
            categories: vec!["cs.DL".into()],
            pdf_url: Some("https://arxiv.org/pdf/2602.00002".into()),
            doi: Some("10.1000/remote-only".into()),
            journal_ref: None,
        };

        database.ensure_remote_paper(&paper)?;

        assert!(database.library_papers()?.is_empty());
        Ok(())
    }

    #[test]
    fn root_filtered_library_only_includes_existing_pdfs_inside_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let root = std::env::temp_dir().join(format!("papr-root-filter-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let inside_path = root.join("inside.pdf");
        fs::write(&inside_path, b"%PDF-1.7\ninside")?;
        let outside_path = root.with_extension("outside.pdf");
        fs::write(&outside_path, b"%PDF-1.7\noutside")?;
        let inside_path_string = inside_path.to_string_lossy().into_owned();
        let outside_path_string = outside_path.to_string_lossy().into_owned();

        database.import_pdf(&imported_pdf(&inside_path_string, "inside-hash"))?;
        database.import_pdf(&imported_pdf(&outside_path_string, "outside-hash"))?;

        let rows = database.library_papers_in_roots(std::slice::from_ref(&root))?;

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].pdf_path.as_deref(),
            Some(inside_path_string.as_str())
        );
        let _ = fs::remove_file(outside_path);
        let _ = fs::remove_dir_all(root);
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
    fn directory_sync_creates_exclusive_collections_and_leaves_root_unassigned()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let root = PathBuf::from("/research/library");
        let mut pdf = imported_pdf("/research/library/GW/paper.pdf", "layout-hash");
        pdf.library_root = Some(root.clone());
        pdf.relative_directory = Some(PathBuf::from("GW"));
        database.import_pdf(&pdf)?;
        let paper_id = database
            .paper_id_for_pdf(&pdf)?
            .ok_or("imported PDF has no paper id")?;
        database.sync_pdf_collection(paper_id, &pdf)?;
        let gw = database.collections()?;
        assert_eq!(gw.len(), 1);
        assert_eq!(gw[0].name, "GW");
        assert_eq!(gw[0].paper_count, 1);

        pdf.path = root.join("ML/paper.pdf");
        pdf.relative_directory = Some(PathBuf::from("ML"));
        database.sync_pdf_collection(paper_id, &pdf)?;
        let collections = database.collections()?;
        assert_eq!(
            collections.iter().map(|item| item.paper_count).sum::<u64>(),
            1
        );
        assert_eq!(
            collections
                .iter()
                .find(|item| item.name == "ML")
                .map(|item| item.paper_count),
            Some(1)
        );

        pdf.path = root.join("paper.pdf");
        pdf.relative_directory = None;
        database.sync_pdf_collection(paper_id, &pdf)?;
        assert_eq!(
            database
                .collections()?
                .iter()
                .map(|item| item.paper_count)
                .sum::<u64>(),
            0
        );
        Ok(())
    }

    #[test]
    fn empty_directory_creates_a_collection() -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        database.sync_collection_directory(&CollectionDirectory {
            library_root: PathBuf::from("/research/library"),
            relative_path: PathBuf::from("Empty Collection"),
        })?;
        let collections = database.collections()?;
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "Empty Collection");
        assert_eq!(collections[0].paper_count, 0);
        Ok(())
    }

    #[test]
    fn renaming_collection_updates_folder_and_member_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let old = PathBuf::from("/library/Old Name");
        let new = PathBuf::from("/library/New Name");
        let mut pdf = imported_pdf("/library/Old Name/paper.pdf", "rename-hash");
        pdf.library_root = Some(PathBuf::from("/library"));
        pdf.relative_directory = Some(PathBuf::from("Old Name"));
        database.import_pdf(&pdf)?;
        let paper_id = database
            .paper_id_for_pdf(&pdf)?
            .ok_or("imported PDF has no paper id")?;
        database.sync_pdf_collection(paper_id, &pdf)?;
        let collection = database.collections()?.remove(0);

        database.rename_collection(collection.id, "New Name", &old, &new)?;
        let renamed = database.collections()?.remove(0);
        assert_eq!(renamed.name, "New Name");
        assert_eq!(renamed.folder_path.as_deref(), Some("/library/New Name"));
        assert_eq!(
            database.library_papers()?[0].pdf_path.as_deref(),
            Some("/library/New Name/paper.pdf")
        );
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

    #[test]
    fn dashboard_feed_cache_is_scoped_by_date_and_keywords()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::in_memory()?;
        let paper = RemotePaper {
            id: "https://arxiv.org/abs/2607.00001".into(),
            title: "Cached daily paper".into(),
            authors: vec!["Researcher".into()],
            abstract_text: "Cached abstract".into(),
            published: Utc::now(),
            updated: Utc::now(),
            categories: vec!["astro-ph".into()],
            pdf_url: None,
            doi: None,
            journal_ref: None,
        };
        database.save_dashboard_feed_cache("2026-07-13", "gravity", &[paper])?;

        let cached = database
            .dashboard_feed_cache("2026-07-13", "gravity")?
            .ok_or("daily cache was not stored")?;
        assert_eq!(cached[0].title, "Cached daily paper");
        assert!(
            database
                .dashboard_feed_cache("2026-07-14", "gravity")?
                .is_none()
        );
        assert!(
            database
                .dashboard_feed_cache("2026-07-13", "different")?
                .is_none()
        );
        Ok(())
    }

    fn imported_pdf(path: &str, hash: &str) -> ImportedPdf {
        ImportedPdf {
            path: PathBuf::from(path),
            title: "Imported title".into(),
            content_hash: hash.into(),
            file_size: 42,
            library_root: None,
            relative_directory: None,
        }
    }
}
