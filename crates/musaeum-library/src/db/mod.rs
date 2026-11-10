//! One writer and several readers, by construction.
//!
//! Every write goes through a single thread that owns the only connection able to write, so
//! writers cannot contend and there is no `SQLITE_BUSY` to handle between them. Readers each have
//! a read-only connection and, in WAL mode, never wait for the writer or hold it up.
//!
//! Two things that would break that promise on Windows are left out on purpose, and the numbers that
//! show why are in `docs/adr/0002-fase-1.md`. Readers do not map the database into memory: with a
//! mapping, reads stalled for up to 1.6 s while a checkpoint ran, against a target of 30 ms, and without
//! one they do not. And checkpoints do not run inside a commit, where they took the only writer about
//! 480 ms out of every thousand pages, but on a thread of their own.

pub(crate) mod backup;
pub(crate) mod migrate;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_fs::enter_background;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::{Error, Result, io_error};
use migrate::Migration;

/// `application_id` of `library.db`: "MUSL".
pub(crate) const LIBRARY_APPLICATION_ID: i32 = 0x4D55_534C;
/// `application_id` of `cache.db`: "MUSC".
pub(crate) const CACHE_APPLICATION_ID: i32 = 0x4D55_5343;

/// The migrations of `library.db`, in order.
pub(crate) const LIBRARY_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../../migrations/library/0001_initial.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../../migrations/library/0002_reading.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("../../migrations/library/0003_flow_reading.sql"),
    },
];

/// The migrations of `cache.db`, in order.
pub(crate) const CACHE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../../migrations/cache/0001_initial.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../../migrations/cache/0002_page_geometry.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("../../migrations/cache/0003_page_content.sql"),
    },
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the checkpointer looks at whether there is anything to do.
const CHECKPOINT_TICK: Duration = Duration::from_millis(500);
/// While writes keep coming, how long the log may go without being written into the database. Once
/// they stop, it happens at the next tick.
const CHECKPOINT_WHILE_BUSY: Duration = Duration::from_secs(5);
/// What the log can reach before the writer stops to bring it to an end. With commits arriving all
/// the time, a checkpoint that lets them through never gets there, and the log would only grow.
const LOG_BYTES_BEFORE_WAITING: u64 = 64 * 1024 * 1024;
/// How much of the file a log that has started over keeps. Less than the above, or this would always
/// look like a log that has grown too long.
const LOG_BYTES_KEPT: i64 = 16 * 1024 * 1024;

/// What is particular to one database file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DatabaseSpec {
    pub application_id: i32,
    pub migrations: &'static [Migration],
    /// `FULL` for what cannot be replaced. With WAL, `NORMAL` can lose the last commits in a power
    /// failure, though never corrupt the file, which is fine for what can be rebuilt.
    pub synchronous: &'static str,
    pub readers: usize,
    pub reader_cache_kib: i64,
}

/// `library.db`.
pub(crate) const LIBRARY_SPEC: DatabaseSpec = DatabaseSpec {
    application_id: LIBRARY_APPLICATION_ID,
    migrations: LIBRARY_MIGRATIONS,
    synchronous: "FULL",
    readers: 4,
    reader_cache_kib: 8 * 1024,
};

/// `cache.db`.
pub(crate) const CACHE_SPEC: DatabaseSpec = DatabaseSpec {
    application_id: CACHE_APPLICATION_ID,
    migrations: CACHE_MIGRATIONS,
    synchronous: "NORMAL",
    readers: 1,
    reader_cache_kib: 1024,
};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// A database file, with its writer and its readers.
pub(crate) struct Database {
    path: PathBuf,
    log: Arc<PathBuf>,
    // These go in the order they have to close: the last connection to close is the one that cleans
    // up the log, and it can only do that if it is able to write.
    _checkpointer: Option<Checkpointer>,
    readers: Readers,
    // `None` when the database was opened only to look at.
    writer: Option<Writer>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Database")
            .field("path", &self.path)
            .field("writable", &self.writer.is_some())
            .finish()
    }
}

impl Database {
    /// Opens the file, creating and upgrading it as needed.
    pub fn open(path: &Path, spec: &DatabaseSpec) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        configure_writer(&connection, spec)?;
        migrate::check_owner(&connection, spec.application_id, path)?;
        migrate::apply(&mut connection, spec.application_id, spec.migrations)?;

        let readers = Readers::open(path, spec)?;
        let writer = Writer::spawn(connection)?;
        let checkpointer = Checkpointer::spawn(path, Arc::clone(&writer.commits))?;
        Ok(Self {
            path: path.to_owned(),
            log: Arc::new(log_of(path)),
            _checkpointer: Some(checkpointer),
            readers,
            writer: Some(writer),
        })
    }

    /// Opens an existing file only to read it. It must already be at the version this program
    /// expects, since upgrading it would be a write.
    pub fn open_read_only(path: &Path, spec: &DatabaseSpec) -> Result<Self> {
        let readers = Readers::open(path, spec)?;
        readers.with(|connection| {
            migrate::check_owner(connection, spec.application_id, path)?;
            match migrate::version_gap(connection, spec.migrations)? {
                0 => Ok(()),
                _ => Err(Error::ReadOnly(
                    "this library needs an upgrade, which takes opening it for writing",
                )),
            }
        })?;
        Ok(Self {
            path: path.to_owned(),
            log: Arc::new(log_of(path)),
            _checkpointer: None,
            readers,
            writer: None,
        })
    }

    /// Runs `work` in a transaction on the writer thread, and waits for it. The transaction takes
    /// the write lock from its first statement, and an error rolls it back.
    pub fn write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(Error::ReadOnly("this library was opened for reading only"))?;
        let commits = Arc::clone(&writer.commits);
        let log = Arc::clone(&self.log);
        writer.call(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = work(&transaction)?;
            transaction.commit()?;
            commits.fetch_add(1, Ordering::Relaxed);
            bring_a_long_log_to_an_end(connection, &log);
            Ok(value)
        })
    }

    /// Runs `work` on one of the read-only connections, waiting if all of them are in use.
    pub fn read<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.readers.with(work)
    }
}

fn log_of(path: &Path) -> PathBuf {
    let mut log = path.as_os_str().to_owned();
    log.push("-wal");
    PathBuf::from(log)
}

/// This is the writer's job and not the checkpointer's, because bringing the log to an end takes the
/// write lock for as long as it lasts. On this thread, that only delays the next write. On any other,
/// the writer could run out of patience and fail with `SQLITE_BUSY`, which is what happened.
fn bring_a_long_log_to_an_end(connection: &Connection, log: &Path) {
    if std::fs::metadata(log).map_or(0, |metadata| metadata.len()) < LOG_BYTES_BEFORE_WAITING {
        return;
    }
    // Waiting for nobody. If the checkpointer is at it, or a reader is in the log, the commit after this
    // one tries again, and nothing depends on this having worked.
    let _ = connection.busy_timeout(Duration::ZERO);
    let _ = connection.query_row("PRAGMA wal_checkpoint(RESTART)", [], |_| Ok(()));
    let _ = connection.busy_timeout(BUSY_TIMEOUT);
}

fn configure_writer(connection: &Connection, spec: &DatabaseSpec) -> Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", spec.synchronous)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    // Keeps the log from staying large after a burst of writes, such as a first scan.
    connection.pragma_update(None, "journal_size_limit", LOG_BYTES_KEPT)?;
    connection.pragma_update(None, "cache_size", -16 * 1024)?;
    // The checkpointer does this, so that a commit never has to.
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.set_prepared_statement_cache_capacity(64);
    Ok(())
}

struct Writer {
    sender: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    commits: Arc<AtomicU64>,
}

impl Writer {
    fn spawn(connection: Connection) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("musaeum-db-writer".to_owned())
            .spawn(move || {
                let mut connection = connection;
                while let Ok(job) = receiver.recv() {
                    job(&mut connection);
                }
            })
            .map_err(io_error("starting the database writer"))?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            commits: Arc::new(AtomicU64::new(0)),
        })
    }

    fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, answer) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |connection| {
            // The caller may have gone, and then nobody wants to know.
            let _ = reply.send(work(connection));
        });
        self.sender
            .as_ref()
            .ok_or(Error::WriterStopped)?
            .send(job)
            .map_err(|_| Error::WriterStopped)?;
        answer.recv().map_err(|_| Error::WriterStopped)?
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Moves what is in the log into the database, away from the writer and at low priority.
struct Checkpointer {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Checkpointer {
    fn spawn(path: &Path, commits: Arc<AtomicU64>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        // Each connection's own setting, and without it this one would checkpoint when it closes too.
        connection.pragma_update(None, "wal_autocheckpoint", 0)?;

        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("musaeum-db-checkpoint".to_owned())
            .spawn(move || {
                let _background = enter_background();
                let mut checkpointed_at = 0;
                let mut seen = 0;
                let mut last_checkpoint = Instant::now();
                loop {
                    match stopped.recv_timeout(CHECKPOINT_TICK) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let now = commits.load(Ordering::Relaxed);
                    let quiet = now == seen;
                    seen = now;
                    if now == checkpointed_at
                        || !(quiet || last_checkpoint.elapsed() >= CHECKPOINT_WHILE_BUSY)
                    {
                        continue;
                    }
                    // This never holds the writer up. If it fails, the log just stays longer for now.
                    let done =
                        connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
                    if done.is_ok() {
                        checkpointed_at = now;
                        last_checkpoint = Instant::now();
                    }
                }
            })
            .map_err(io_error("starting the checkpoints"))?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Readers {
    idle: Mutex<Vec<Connection>>,
    returned: Condvar,
}

impl Readers {
    fn open(path: &Path, spec: &DatabaseSpec) -> Result<Self> {
        let mut connections = Vec::with_capacity(spec.readers);
        for _ in 0..spec.readers {
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            connection.busy_timeout(BUSY_TIMEOUT)?;
            connection.pragma_update(None, "query_only", true)?;
            connection.pragma_update(None, "temp_store", "MEMORY")?;
            connection.pragma_update(None, "cache_size", -spec.reader_cache_kib)?;
            connection.set_prepared_statement_cache_capacity(32);
            connections.push(connection);
        }
        Ok(Self {
            idle: Mutex::new(connections),
            returned: Condvar::new(),
        })
    }

    fn with<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let lease = self.lease();
        work(lease.connection())
    }

    fn lease(&self) -> Lease<'_> {
        let mut idle = lock(&self.idle);
        loop {
            if let Some(connection) = idle.pop() {
                return Lease {
                    readers: self,
                    connection: Some(connection),
                };
            }
            idle = self
                .returned
                .wait(idle)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

// Nothing done while holding this can leave the list in a bad state, so a panic elsewhere is no
// reason to stop reading.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A connection, given back when this ends, however it ends.
struct Lease<'a> {
    readers: &'a Readers,
    connection: Option<Connection>,
}

impl Lease<'_> {
    fn connection(&self) -> &Connection {
        self.connection.as_ref().expect("held until dropped")
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            lock(&self.readers.idle).push(connection);
            self.readers.returned.notify_one();
        }
    }
}
