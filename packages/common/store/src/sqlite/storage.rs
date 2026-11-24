use std::collections::BTreeSet;

use sqlx::{
    Pool, Row,
    migrate::MigrateDatabase,
    sqlite::{
        Sqlite, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    },
};

use super::migrations::{get_migrator, try_repair_migration_3_checksum};
use crate::error::StoreResult;

/// Macro-free SQLite workspace storage engine compatible with 0.27.4 / 0.27.5 in-place databases.
pub struct SqliteDocStorage {
    pub(super) pool: Pool<Sqlite>,
    path: String,
}

impl SqliteDocStorage {
    /// Prepare a lazy SQLite connection pool configured with WAL, `synchronous = NORMAL`,
    /// and `foreign_keys = ON`.
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let base_options = SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true)
            .synchronous(SqliteSynchronous::Normal);

        if path == ":memory:" {
            let pool = SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None)
                .connect_lazy_with(base_options);
            Self { pool, path }
        } else {
            let pool = SqlitePoolOptions::new()
                .max_connections(4)
                .connect_lazy_with(base_options.journal_mode(SqliteJournalMode::Wal));
            Self { pool, path }
        }
    }

    /// Return the underlying database path string.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Create the database if missing and apply the frozen 5-step migrations,
    /// repairing the historical migration #3 trailing-space checksum if needed.
    pub async fn connect(&self) -> StoreResult<()> {
        if self.path != ":memory:" && !Sqlite::database_exists(&self.path).await? {
            Sqlite::create_database(&self.path).await?;
        }
        self.migrate().await?;
        Ok(())
    }

    async fn migrate(&self) -> StoreResult<()> {
        let migrator = get_migrator();
        if let Err(err) = migrator.run(&self.pool).await {
            if matches!(err, sqlx::migrate::MigrateError::VersionMismatch(3))
                && try_repair_migration_3_checksum(&self.pool, &migrator).await?
            {
                migrator.run(&self.pool).await?;
            } else {
                return Err(err.into());
            }
        }
        Ok(())
    }

    /// Close the connection pool cleanly.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Return whether the connection pool has been closed.
    pub fn is_closed(&self) -> bool {
        self.pool.is_closed()
    }

    /// Flush the WAL journal into the main database file.
    pub async fn checkpoint(&self) -> StoreResult<()> {
        sqlx::query("PRAGMA wal_checkpoint(FULL);")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Export a compacted copy of the database to `target_path`.
    pub async fn vacuum_into(&self, target_path: &str) -> StoreResult<()> {
        sqlx::query("VACUUM INTO ?;")
            .bind(target_path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Verify that all required v2/v5 tables exist on the connected database.
    pub async fn validate_required_tables(&self) -> StoreResult<bool> {
        let rows = sqlx::query("SELECT name FROM sqlite_schema WHERE type = 'table'")
            .fetch_all(&self.pool)
            .await?;
        let mut names = BTreeSet::new();
        for row in rows {
            let name: String = row.try_get("name")?;
            names.insert(name);
        }
        let required = [
            "meta",
            "snapshots",
            "updates",
            "clocks",
            "blobs",
            "peer_clocks",
            "peer_blob_sync",
            "idx_snapshots",
            "indexer_sync",
        ];
        Ok(required.iter().all(|table| names.contains(*table)))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use sqlx::migrate::{Migration, Migrator};

    use super::*;
    use crate::sqlite::{SetBlob, millis_to_naive_utc};

    #[tokio::test]
    async fn upgrades_0274_db_with_legacy_migration_3_checksum_and_preserves_data() {
        let temp_path = std::env::temp_dir().join(format!(
            "affine-store-upgrade-test-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_file(format!("{}-wal", temp_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", temp_path.display()));

        let path_str = temp_path.to_string_lossy().to_string();
        let storage = SqliteDocStorage::new(&path_str);
        Sqlite::create_database(&path_str).await.unwrap();

        // Simulate 0.27.4: only 4 migrations applied, and migration #3 had a trailing space.
        let full_migrator = get_migrator();
        let mut legacy_migrations = full_migrator.migrations.to_vec();
        let mig3 = legacy_migrations[2].clone();
        legacy_migrations[2] = Migration::new(
            mig3.version,
            mig3.description.clone(),
            mig3.migration_type,
            Cow::Owned(format!("{} ", mig3.sql)),
            mig3.no_tx,
        );
        legacy_migrations.truncate(4);
        let legacy_migrator = Migrator {
            migrations: Cow::Owned(legacy_migrations),
            ..Migrator::DEFAULT
        };
        legacy_migrator.run(&storage.pool).await.unwrap();

        // Seed 0.27.4 data before upgrading to version 5.
        storage.set_space_id("ws-0274").await.unwrap();
        let doc_ts = storage
            .push_update("doc-legacy", [0u8, 0u8], 1_700_000_000_000)
            .await
            .unwrap();
        storage
            .set_blob(SetBlob {
                key: "blob-1".to_string(),
                data: vec![1, 2, 3, 4],
                mime: "application/octet-stream".to_string(),
            })
            .await
            .unwrap();
        storage
            .set_peer_remote_clock(
                "cloud",
                "doc-legacy",
                millis_to_naive_utc(1_700_000_000_500),
            )
            .await
            .unwrap();
        storage.close().await;

        // Reopen with current 5-step migrator: auto-repairs #3 and upgrades to #5.
        let upgraded = SqliteDocStorage::new(&path_str);
        upgraded.connect().await.unwrap();
        assert!(upgraded.validate_required_tables().await.unwrap());

        // Verify PRAGMA settings.
        let journal: String = sqlx::query("PRAGMA journal_mode;")
            .fetch_one(&upgraded.pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        let fk: i64 = sqlx::query("PRAGMA foreign_keys;")
            .fetch_one(&upgraded.pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(fk, 1);

        // Verify migration #3 checksum repaired and migration #5 applied.
        let max_version: i64 = sqlx::query("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&upgraded.pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(max_version, 5);

        // Verify all 0.27.4 data survived losslessly.
        let rec = upgraded.get_doc("doc-legacy").await.unwrap().unwrap();
        assert_eq!(rec.timestamp, doc_ts);
        let blob = upgraded.get_blob("blob-1").await.unwrap().unwrap();
        assert_eq!(blob.data, vec![1, 2, 3, 4]);
        let clock = upgraded
            .get_peer_remote_clock("cloud", "doc-legacy")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(clock.timestamp, millis_to_naive_utc(1_700_000_000_500));

        // Verify WAL checkpoint and reopen durability.
        upgraded.checkpoint().await.unwrap();
        upgraded.close().await;

        let reopened = SqliteDocStorage::new(&path_str);
        reopened.connect().await.unwrap();
        assert!(reopened.get_doc("doc-legacy").await.unwrap().is_some());
        reopened.close().await;

        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_file(format!("{}-wal", temp_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", temp_path.display()));
    }

    #[tokio::test]
    async fn interleaved_connections_and_uncheckpointed_wal_recovery() {
        let temp_path = std::env::temp_dir().join(format!(
            "affine-store-wal-recovery-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_file(format!("{}-wal", temp_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", temp_path.display()));

        let path_str = temp_path.to_string_lossy().to_string();
        let writer_a = SqliteDocStorage::new(&path_str);
        writer_a.connect().await.unwrap();
        let writer_b = SqliteDocStorage::new(&path_str);
        writer_b.connect().await.unwrap();

        writer_a
            .push_update("shared-doc", [0u8, 0u8], 1_000)
            .await
            .unwrap();
        writer_b
            .set_blob(SetBlob {
                key: "shared-blob".into(),
                data: vec![7, 8, 9],
                mime: "application/octet-stream".into(),
            })
            .await
            .unwrap();
        assert!(writer_a.get_blob("shared-blob").await.unwrap().is_some());
        assert!(writer_b.get_doc("shared-doc").await.unwrap().is_some());

        // Drop without checkpointing to simulate abrupt crash with WAL frames pending.
        drop(writer_a);
        drop(writer_b);

        let recovered = SqliteDocStorage::new(&path_str);
        recovered.connect().await.unwrap();
        assert!(recovered.get_doc("shared-doc").await.unwrap().is_some());
        assert_eq!(
            recovered
                .get_blob("shared-blob")
                .await
                .unwrap()
                .unwrap()
                .data,
            vec![7, 8, 9]
        );
        recovered.close().await;

        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_file(format!("{}-wal", temp_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", temp_path.display()));
    }
}
