//! Versioned changes to a database's schema.
//!
//! `user_version` says which ones a file already has. Each runs in its own transaction, along with
//! the new version number, so a migration either happened completely or not at all.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::error::{Error, Result};

/// One change to the schema.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Migration {
    /// What `user_version` becomes. The first is 1, and each one follows the last.
    pub version: i64,
    /// What to run.
    pub sql: &'static str,
}

/// The newest version in a list of migrations.
pub(crate) fn latest(migrations: &[Migration]) -> i64 {
    migrations.last().map_or(0, |migration| migration.version)
}

fn user_version(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Refuses a file that is a database but not one of ours. A file with nothing in it yet is fine.
pub(crate) fn check_owner(connection: &Connection, application_id: i32, path: &Path) -> Result<()> {
    let found: i32 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if found == application_id {
        return Ok(());
    }
    let tables: i64 =
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
    if found == 0 && tables == 0 {
        return Ok(());
    }
    Err(Error::NotOurDatabase(path.to_owned()))
}

/// How many versions behind `migrations` the file is, or an error if it is ahead.
pub(crate) fn version_gap(connection: &Connection, migrations: &[Migration]) -> Result<i64> {
    let found = user_version(connection)?;
    let known = latest(migrations);
    if found > known {
        return Err(Error::NewerSchema { found, known });
    }
    Ok(known - found)
}

/// The version of the file at `path`, without changing anything: `None` if there is no such file,
/// and 0 if it has nothing in it yet.
pub(crate) fn peek_version(path: &Path, application_id: i32) -> Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    check_owner(&connection, application_id, path)?;
    Ok(Some(user_version(&connection)?))
}

/// Brings the file up to date, returning how many migrations ran. A file newer than this program
/// is refused untouched: it might not understand what is there, and writing could destroy it.
pub(crate) fn apply(
    connection: &mut Connection,
    application_id: i32,
    migrations: &[Migration],
) -> Result<usize> {
    let found = user_version(connection)?;
    version_gap(connection, migrations)?;
    let mut applied = 0;
    for migration in migrations
        .iter()
        .filter(|migration| migration.version > found)
    {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(migration.sql)?;
        transaction.pragma_update(None, "user_version", migration.version)?;
        transaction.pragma_update(None, "application_id", application_id)?;
        transaction.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: i32 = 0x1234;
    const FIRST: Migration = Migration {
        version: 1,
        sql: "CREATE TABLE a (x INTEGER);",
    };
    const SECOND: Migration = Migration {
        version: 2,
        sql: "ALTER TABLE a ADD COLUMN y INTEGER DEFAULT 7;",
    };
    const BROKEN: Migration = Migration {
        version: 2,
        sql: "ALTER TABLE a ADD COLUMN y INTEGER; INSERT INTO nonexistent VALUES (1);",
    };

    fn database() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn a_new_file_gets_everything() {
        let mut connection = database();
        assert_eq!(apply(&mut connection, ID, &[FIRST, SECOND]).unwrap(), 2);
        assert_eq!(user_version(&connection).unwrap(), 2);
        let id: i32 = connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .unwrap();
        assert_eq!(id, ID);
    }

    #[test]
    fn an_upgrade_keeps_the_data_and_runs_only_what_is_new() {
        let mut connection = database();
        apply(&mut connection, ID, &[FIRST]).unwrap();
        connection
            .execute("INSERT INTO a (x) VALUES (42)", [])
            .unwrap();

        assert_eq!(apply(&mut connection, ID, &[FIRST, SECOND]).unwrap(), 1);
        let row: (i64, i64) = connection
            .query_row("SELECT x, y FROM a", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(row, (42, 7));
        assert_eq!(apply(&mut connection, ID, &[FIRST, SECOND]).unwrap(), 0);
    }

    #[test]
    fn a_failing_migration_leaves_no_trace() {
        let mut connection = database();
        apply(&mut connection, ID, &[FIRST]).unwrap();
        connection
            .execute("INSERT INTO a (x) VALUES (42)", [])
            .unwrap();

        assert!(apply(&mut connection, ID, &[FIRST, BROKEN]).is_err());

        assert_eq!(user_version(&connection).unwrap(), 1);
        let columns: i64 = connection
            .query_row("SELECT count(*) FROM pragma_table_info('a')", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(columns, 1, "the half-done migration was rolled back");
        let x: i64 = connection
            .query_row("SELECT x FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(x, 42);
    }

    #[test]
    fn a_file_from_a_newer_version_is_refused_untouched() {
        let mut connection = database();
        apply(&mut connection, ID, &[FIRST, SECOND]).unwrap();
        assert!(matches!(
            apply(&mut connection, ID, &[FIRST]),
            Err(Error::NewerSchema { found: 2, known: 1 })
        ));
        assert_eq!(user_version(&connection).unwrap(), 2);
    }

    #[test]
    fn someone_elses_database_is_refused() {
        let connection = database();
        connection
            .execute_batch("CREATE TABLE theirs (x);")
            .unwrap();
        assert!(matches!(
            check_owner(&connection, ID, Path::new("x.db")),
            Err(Error::NotOurDatabase(_))
        ));
        let empty = database();
        check_owner(&empty, ID, Path::new("y.db")).unwrap();
    }
}
