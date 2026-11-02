//! Keeps a library up to date while the program runs.
//!
//! Three things can tell the indexer to look: starting up, the clock and the system's change
//! events. All they do is say where to look. What was found out comes from [`Library::reconcile`],
//! so an event that is wrong, late or lost costs some delay and nothing more.
//!
//! The watches start before the first scan, so anything that changes during that scan is reported.
//! And the scan and the hashing each have a low-priority thread, so nothing here makes the rest of
//! the program wait.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_fs::{
    DEFAULT_WATCH_BUFFER, DirectoryWatch, WatchEvent, enter_background, watch_directory,
};

use crate::content::ContentReport;
use crate::error::{Result, io_error};
use crate::identity::ReconcileReport;
use crate::library::Library;
use crate::model::RootId;
use crate::reconcile::ReconcileScope;
use crate::scan::format_of;

/// How an indexer behaves.
#[derive(Clone, Copy, Debug)]
pub struct IndexerConfig {
    /// How long things must be quiet before looking. Saving a file usually takes several steps, and
    /// looking in the middle of them would see something that was never meant to exist.
    pub settle: Duration,
    /// The longest it will wait for things to be quiet, for when they never are.
    pub max_delay: Duration,
    /// How often to compare everything anyway, whatever the events have said.
    pub reconcile_interval: Duration,
    /// How often to check whether a backup is due.
    pub backup_check_interval: Duration,
    /// How much can happen between two looks at a watch before the system loses track.
    pub watch_buffer: usize,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            settle: Duration::from_millis(750),
            max_delay: Duration::from_secs(5),
            reconcile_interval: Duration::from_secs(15 * 60),
            backup_check_interval: Duration::from_secs(60 * 60),
            watch_buffer: DEFAULT_WATCH_BUFFER,
        }
    }
}

/// What an indexer has to report.
#[derive(Clone, Debug)]
pub enum IndexerEvent {
    /// A scan ended.
    Reconciled(ReconcileReport),
    /// A pass over what was waiting for its hash or metadata ended.
    ContentProcessed(ContentReport),
    /// The system lost track of changes under a root, so all of it gets looked at again.
    WatchOverflowed(RootId),
    /// A root's watch ended, and will start again once there is something to watch.
    WatchLost(RootId, String),
    /// Something went wrong while doing the above, and it will be tried again.
    Failed(String),
}

enum Control {
    Changed(RootId, PathBuf),
    Overflowed(RootId),
    Lost(RootId, String),
    ReconcileNow,
    Stop,
}

#[derive(Debug, Default)]
struct Activity {
    scan_pending: bool,
    scanning: bool,
    content_pending: bool,
    processing: bool,
    stopping: bool,
}

impl Activity {
    fn is_idle(&self) -> bool {
        !(self.scan_pending || self.scanning || self.content_pending || self.processing)
    }
}

#[derive(Debug, Default)]
struct Shared {
    activity: Mutex<Activity>,
    changed: Condvar,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut Activity)) {
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut activity);
        self.changed.notify_all();
    }
}

/// A library being kept up to date. Dropping this stops it, waiting for what is underway to end.
#[derive(Debug)]
pub struct Indexer {
    control: Sender<Control>,
    shared: Arc<Shared>,
    library: Arc<Library>,
    coordinator: Option<JoinHandle<()>>,
    content: Option<JoinHandle<()>>,
}

impl Indexer {
    /// Starts everything, beginning with a scan of every root.
    pub fn start(
        library: Arc<Library>,
        config: IndexerConfig,
        events: Option<Sender<IndexerEvent>>,
    ) -> Result<Self> {
        let (control, inbox) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        shared.update(|activity| activity.scan_pending = true);

        let content = {
            let library = Arc::clone(&library);
            let shared = Arc::clone(&shared);
            let events = events.clone();
            thread::Builder::new()
                .name("musaeum-content".to_owned())
                .spawn(move || run_content(&library, &shared, events.as_ref()))
                .map_err(io_error("starting the hashing thread"))?
        };
        let coordinator = {
            let mut coordinator = Coordinator {
                library: Arc::clone(&library),
                config,
                shared: Arc::clone(&shared),
                events,
                control: control.clone(),
                watches: HashMap::new(),
            };
            thread::Builder::new()
                .name("musaeum-indexer".to_owned())
                .spawn(move || coordinator.run(&inbox))
                .map_err(io_error("starting the indexer"))?
        };
        Ok(Self {
            control,
            shared,
            library,
            coordinator: Some(coordinator),
            content: Some(content),
        })
    }

    /// Looks at everything now, including any root added since the last time.
    pub fn reconcile_now(&self) {
        self.shared.update(|activity| activity.scan_pending = true);
        let _ = self.control.send(Control::ReconcileNow);
    }

    /// Waits until nothing is underway or waiting, or `timeout` runs out, and says which. A change
    /// the system has not reported yet is not something this can know about.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut activity = self
            .shared
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !activity.is_idle() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            activity = self
                .shared
                .changed
                .wait_timeout(activity, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        true
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.library.request_stop();
        self.shared.update(|activity| activity.stopping = true);
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.coordinator.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.content.take() {
            let _ = thread.join();
        }
        self.library.clear_stop();
    }
}

fn run_content(library: &Library, shared: &Shared, events: Option<&Sender<IndexerEvent>>) {
    let _background = enter_background();
    loop {
        {
            let mut activity = shared
                .activity
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !activity.content_pending && !activity.stopping {
                activity = shared
                    .changed
                    .wait(activity)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if activity.stopping {
                return;
            }
            activity.content_pending = false;
            activity.processing = true;
        }
        shared.changed.notify_all();

        let event = match library.process_content() {
            Ok(report) => IndexerEvent::ContentProcessed(report),
            Err(error) => IndexerEvent::Failed(format!("hashing: {error}")),
        };
        if let Some(events) = events {
            let _ = events.send(event);
        }
        shared.update(|activity| activity.processing = false);
    }
}

struct Coordinator {
    library: Arc<Library>,
    config: IndexerConfig,
    shared: Arc<Shared>,
    events: Option<Sender<IndexerEvent>>,
    control: Sender<Control>,
    watches: HashMap<RootId, DirectoryWatch>,
}

/// What a root's next look is.
#[derive(Default)]
struct Dirty {
    directories: HashSet<PathBuf>,
    everything: bool,
}

impl Coordinator {
    fn run(&mut self, inbox: &mpsc::Receiver<Control>) {
        let _background = enter_background();

        let mut dirty: HashMap<RootId, Dirty> = HashMap::new();
        let mut first_change: Option<Instant> = None;
        let mut last_change = Instant::now();
        let mut next_full = Instant::now();
        let mut next_backup = Instant::now();

        loop {
            let now = Instant::now();
            if now >= next_full {
                self.look_at_everything();
                next_full = Instant::now() + self.config.reconcile_interval;
                dirty.clear();
                first_change = None;
            }
            if now >= next_backup {
                self.backup_if_due();
                next_backup = now + self.config.backup_check_interval;
            }

            let settled = first_change.is_some_and(|first| {
                now.duration_since(last_change) >= self.config.settle
                    || now.duration_since(first) >= self.config.max_delay
            });
            if settled {
                let batch = std::mem::take(&mut dirty);
                first_change = None;
                self.look_at(batch);
                self.shared.update(|activity| activity.scan_pending = false);
                continue;
            }

            let mut wake_at = next_full.min(next_backup);
            if let Some(first) = first_change {
                wake_at = wake_at
                    .min(last_change + self.config.settle)
                    .min(first + self.config.max_delay);
            }
            match inbox.recv_timeout(wake_at.saturating_duration_since(Instant::now())) {
                Ok(Control::Changed(root, path)) => {
                    if let Some(directories) = self.directories_to_look_at(&path) {
                        let entry = dirty.entry(root).or_default();
                        entry.directories.extend(directories);
                        last_change = Instant::now();
                        first_change.get_or_insert(last_change);
                        self.shared.update(|activity| activity.scan_pending = true);
                    }
                }
                Ok(Control::Overflowed(root)) => {
                    self.report(IndexerEvent::WatchOverflowed(root));
                    dirty.entry(root).or_default().everything = true;
                    last_change = Instant::now();
                    first_change.get_or_insert(last_change);
                    self.shared.update(|activity| activity.scan_pending = true);
                }
                Ok(Control::Lost(root, reason)) => {
                    self.report(IndexerEvent::WatchLost(root, reason));
                    self.watches.remove(&root);
                    dirty.entry(root).or_default().everything = true;
                    last_change = Instant::now();
                    first_change.get_or_insert(last_change);
                    self.shared.update(|activity| activity.scan_pending = true);
                }
                Ok(Control::ReconcileNow) => next_full = Instant::now(),
                Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn report(&self, event: IndexerEvent) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    /// What a change at this path means to look at, which for most paths that are not books and not
    /// folders is nothing at all. The parent is what will show if the path came or went.
    fn directories_to_look_at(&self, path: &Path) -> Option<Vec<PathBuf>> {
        let parent = path.parent()?.to_owned();
        let name = path.file_name()?;
        match self.library.probe().stat(path) {
            Ok(stat) if stat.is_directory() => Some(vec![parent, path.to_owned()]),
            Ok(_) if format_of(name).is_none() => None,
            _ => Some(vec![parent]),
        }
    }

    fn look_at_everything(&mut self) {
        self.sync_watches();
        let roots = match self.library.roots() {
            Ok(roots) => roots,
            Err(error) => {
                self.report(IndexerEvent::Failed(format!("reading the roots: {error}")));
                return;
            }
        };
        let batch = roots
            .into_iter()
            .map(|root| {
                (
                    root.id,
                    Dirty {
                        everything: true,
                        ..Dirty::default()
                    },
                )
            })
            .collect();
        self.look_at(batch);
        self.shared.update(|activity| activity.scan_pending = false);
    }

    fn look_at(&mut self, batch: HashMap<RootId, Dirty>) {
        self.shared.update(|activity| activity.scanning = true);
        for (root, dirty) in batch {
            let scope = if dirty.everything {
                ReconcileScope::Everything
            } else {
                ReconcileScope::Directories(dirty.directories)
            };
            match self.library.reconcile(root, &scope) {
                Ok(report) => self.report(IndexerEvent::Reconciled(report)),
                Err(error) => {
                    self.report(IndexerEvent::Failed(format!(
                        "scanning root {root}: {error}"
                    )));
                }
            }
        }
        self.sync_watches();
        let pending = self.library.has_pending_content().unwrap_or(false);
        self.shared.update(|activity| {
            activity.scanning = false;
            activity.content_pending |= pending;
        });
    }

    /// Every root that can be watched is watched, and nothing else is.
    fn sync_watches(&mut self) {
        let Ok(roots) = self.library.roots() else {
            return;
        };
        let available: HashSet<RootId> = roots
            .iter()
            .filter(|root| root.status == "available")
            .map(|root| root.id)
            .collect();
        self.watches.retain(|root, _| available.contains(root));
        for root in roots {
            if root.status != "available" || self.watches.contains_key(&root.id) {
                continue;
            }
            let id = root.id;
            let control = self.control.clone();
            let watch = watch_directory(&root.path, self.config.watch_buffer, move |event| {
                let message = match event {
                    WatchEvent::Changed(path) => Control::Changed(id, path),
                    WatchEvent::Overflow => Control::Overflowed(id),
                    WatchEvent::Lost(error) => Control::Lost(id, error.to_string()),
                };
                let _ = control.send(message);
            });
            match watch {
                Ok(watch) => {
                    self.watches.insert(id, watch);
                }
                Err(error) => self.report(IndexerEvent::WatchLost(id, error.to_string())),
            }
        }
    }

    fn backup_if_due(&self) {
        if let Err(error) = self.library.backup_if_due() {
            self.report(IndexerEvent::Failed(format!("making a backup: {error}")));
        }
    }
}
