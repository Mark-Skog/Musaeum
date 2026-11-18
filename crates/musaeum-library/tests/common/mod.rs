//! What the tests of the library have in common: real folders on a real disk, the real worker.

#![allow(dead_code, reason = "each test file uses part of this")]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gen_fixtures::books::{EpubMeta, epub_bytes, pdf_bytes};
use musaeum_fs::{ContentFile, FileId, Listing, OpenError, ReadPattern, Stat};
use musaeum_library::{
    ContentReport, DocumentId, DocumentInfo, FsProbe, IngestConfig, InstanceId, InstanceInfo,
    InstanceState, Library, LibraryOptions, LibraryPaths, RealFileSystem, ReconcileReport,
    ReconcileScope, RevisionId, RootId, WorkId,
};
use musaeum_sandbox::Policy;

/// A folder that is removed at the end of the test.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("musaeum-test-{}-{number}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        // Windows may report the path in another form, and paths get compared in here.
        let path = fs::canonicalize(&path).unwrap();
        let text = path.to_string_lossy().into_owned();
        let plain = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
        Self(PathBuf::from(plain))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The real worker in the real sandbox, built along with these tests.
pub fn ingest_config() -> IngestConfig {
    IngestConfig {
        exe: Some(PathBuf::from(env!("CARGO_BIN_EXE_musaeum-ingest"))),
        policy: Policy::LockdownParser,
        ..IngestConfig::default()
    }
}

pub fn options() -> LibraryOptions {
    LibraryOptions {
        ingest: ingest_config(),
        ..LibraryOptions::default()
    }
}

/// A library with one root, in temporary folders.
pub struct Fixture {
    // First, because fields are dropped in order, and an open library would keep its folder from going.
    library: Option<Library>,
    pub dir: TempDir,
    pub root: PathBuf,
    pub root_id: RootId,
    pub paths: LibraryPaths,
    pub options: LibraryOptions,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_options(options())
    }

    pub fn with_options(options: LibraryOptions) -> Self {
        let dir = TempDir::new();
        let root = dir.0.join("books");
        fs::create_dir(&root).unwrap();
        let paths = LibraryPaths::in_directory(&dir.0.join("library"));
        let library = Library::open(paths.clone(), options.clone()).unwrap();
        let root_id = library.add_root(&root, &[]).unwrap().id;
        Self {
            dir,
            root,
            root_id,
            paths,
            options,
            library: Some(library),
        }
    }

    pub fn library(&self) -> &Library {
        self.library.as_ref().expect("the library is open")
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Writes a file under the root, creating folders on the way.
    pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    /// Closes the library, as when the program ends.
    pub fn close(&mut self) {
        self.library = None;
    }

    /// Opens it again.
    pub fn reopen(&mut self) {
        self.library = Some(Library::open(self.paths.clone(), self.options.clone()).unwrap());
    }

    /// A scan, without what comes after it.
    pub fn reconcile(&self) -> ReconcileReport {
        self.library()
            .reconcile(self.root_id, &ReconcileScope::Everything)
            .unwrap()
    }

    /// Hashing and metadata for whatever is waiting.
    pub fn process(&self) -> ContentReport {
        self.library().process_content().unwrap()
    }

    /// A scan and all that follows it.
    pub fn settle(&self) -> (ReconcileReport, ContentReport) {
        let scan = self.reconcile();
        let content = self.process();
        (scan, content)
    }

    pub fn instance(&self, path: &Path) -> InstanceInfo {
        self.library()
            .instance_at(path)
            .unwrap()
            .unwrap_or_else(|| panic!("the library has nothing at {}", path.display()))
    }

    pub fn document_at(&self, path: &Path) -> DocumentInfo {
        let document = self.instance(path).document_id;
        self.library().document(document).unwrap()
    }

    /// Who a file is, as far as the library is concerned.
    pub fn identity(&self, path: &Path) -> Identity {
        let instance = self.instance(path);
        Identity {
            work: instance.work_id,
            document: instance.document_id,
            revision: instance.revision_id,
            instance: instance.id,
            highlight: None,
        }
    }

    /// Annotations and a rating: the kind of thing losing a file's identity would lose. One note
    /// about the whole document, and one highlight anchored on its first page.
    pub fn mark_as_cared_for(&self, path: &Path) -> Identity {
        let mut identity = self.identity(path);
        self.library()
            .add_annotation(identity.document, "my note")
            .unwrap();
        let highlight = self
            .library()
            .create_annotation(anchored_highlight(identity.document, identity.revision))
            .unwrap();
        identity.highlight =
            Some(u128::from_str_radix(&highlight.uuid.replace('-', ""), 16).unwrap());
        self.library()
            .edit_work(
                identity.work,
                musaeum_library::WorkEdit {
                    rating: Some(Some(5)),
                    ..musaeum_library::WorkEdit::default()
                },
            )
            .unwrap();
        identity
    }

    /// Fails unless what a person put on this work and document is still there.
    pub fn assert_still_cared_for(&self, identity: &Identity) {
        let document = self.library().document(identity.document).unwrap();
        assert_eq!(document.annotations.len(), 2, "an annotation is gone");
        assert_eq!(document.annotations[0].note, "my note");
        if let Some(uuid) = identity.highlight {
            let hex = format!("{uuid:032x}");
            let uuid = format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            );
            let kept = self.library().annotation(&uuid).unwrap();
            let made = anchored_highlight(identity.document, identity.revision);
            assert_eq!(
                kept.document_id, identity.document,
                "the highlight left its document"
            );
            assert_eq!(kept.deleted_at, None);
            assert_eq!(kept.quote, made.quote, "the quote changed");
            assert_eq!(kept.note, made.note);
            assert_eq!(kept.segments.len(), 1, "a segment is gone");
            let selectors = &kept.segments[0].selectors;
            assert_eq!(
                selectors.len(),
                made.segments[0].selectors.len(),
                "a selector is gone"
            );
            let quote = selectors
                .iter()
                .find(|selector| selector.kind == musaeum_library::SelectorKind::TextQuote)
                .expect("the quote selector");
            assert_eq!(quote.data, made.segments[0].selectors[2].data);
        }
        let work = self.library().work(identity.work).unwrap();
        assert_eq!(work.rating, Some(5), "the rating is gone");
    }

    pub fn state_of(&self, path: &Path) -> InstanceState {
        self.instance(path).state
    }

    /// How many documents the library has, which goes up when it loses track of one.
    pub fn document_count(&self) -> u64 {
        self.library().status().unwrap().documents
    }

    /// A connection of this test's own, to look at what the library's API does not show.
    pub fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.paths.library_db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }
}

/// Who a file is, as far as the library is concerned.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    pub work: WorkId,
    pub document: DocumentId,
    pub revision: RevisionId,
    pub instance: InstanceId,
    /// The UUID of the highlight `mark_as_cared_for` made, as a number so this stays `Copy`.
    pub highlight: Option<u128>,
}

/// Two identities are the same file when the work, document, revision and instance are: the
/// highlight made on it is what is checked separately, in `assert_still_cared_for`.
impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        (self.work, self.document, self.revision, self.instance)
            == (other.work, other.document, other.revision, other.instance)
    }
}

impl Eq for Identity {}

/// A highlight on the first page, with the three selectors a fixed layout's annotation has.
pub fn anchored_highlight(
    document: DocumentId,
    revision: RevisionId,
) -> musaeum_library::NewAnnotation {
    use musaeum_library::{
        AnchorState, AnnotationKind, QuoteSource, Segment, Selector, SelectorKind,
    };
    use serde_json::json;
    let selector = |kind, priority, data| Selector {
        kind,
        data,
        priority,
        normalizer_version: Some(1),
        revision_id: Some(revision),
    };
    musaeum_library::NewAnnotation {
        document_id: document,
        revision_id: revision,
        kind: AnnotationKind::Highlight,
        color: "green".to_owned(),
        note: "**worth** keeping".to_owned(),
        quote: Some("a quoted passage".to_owned()),
        quote_source: QuoteSource::TextLayer,
        progression: Some(0.0),
        anchor_state: AnchorState::Exact,
        resolved_version: Some(1),
        pdf_nm: None,
        segments: vec![Segment {
            ordinal: 0,
            page: Some(0),
            href: None,
            clip: None,
            anchor_state: AnchorState::Exact,
            selectors: vec![
                selector(
                    SelectorKind::PageCharRange,
                    0,
                    json!({"page": 0, "start": 10, "end": 26}),
                ),
                selector(
                    SelectorKind::PageQuads,
                    1,
                    json!({"page": 0, "quads": [[0.1, 0.1, 0.4, 0.1, 0.4, 0.12, 0.1, 0.12]]}),
                ),
                Selector {
                    kind: SelectorKind::TextQuote,
                    data: json!({"exact": "a quoted passage", "prefix": "This is ", "suffix": " on a page"}),
                    priority: 2,
                    normalizer_version: None,
                    revision_id: None,
                },
            ],
        }],
    }
}

/// A small, real PDF with bytes of its own.
pub fn pdf(seed: u64) -> Vec<u8> {
    pdf_bytes(None, 2000, seed)
}

/// A small, real PDF with this `/ID[0]`.
pub fn pdf_with_id(id: &str, seed: u64) -> Vec<u8> {
    pdf_bytes(Some(id), 2000, seed)
}

/// A small, real EPUB.
pub fn epub(title: &str, creators: &[&str], identifier: Option<&str>, seed: u64) -> Vec<u8> {
    epub_bytes(
        &EpubMeta {
            title,
            creators,
            identifier,
            language: Some("pt-BR"),
            date: Some("1899-01-01"),
        },
        2000,
        seed,
    )
}

/// Waits for `condition`, for up to `timeout`.
pub fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Saving the way careful programs do: the new contents go to a temporary file, which then takes
/// the place of the old one in a single step. The file at that path afterwards is a different file.
pub fn safe_save(path: &Path, contents: &[u8]) {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".saving");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, contents).unwrap();
    fs::rename(&temporary, path).unwrap();
}

/// A file's File ID as Windows reports it.
pub fn file_id_of(path: &Path) -> FileId {
    musaeum_fs::stat(path).unwrap().file_id.unwrap()
}

/// Reports what a test machine cannot produce on demand. Anything under a redirected folder appears
/// to be on another volume, with other File IDs. The files and the bytes are real. Only the
/// identity Windows gives them is replaced.
#[derive(Default)]
pub struct RedirectingProbe {
    /// Folders that seem to be on another volume, and the serial number of that volume.
    pub volumes: Mutex<Vec<(PathBuf, u64)>>,
    /// Files whose File ID seems to be something else, by name.
    pub file_ids: Mutex<Vec<(String, FileId)>>,
}

impl RedirectingProbe {
    fn fake_volume(&self, path: &Path) -> Option<u64> {
        self.volumes
            .lock()
            .unwrap()
            .iter()
            .find(|(folder, _)| path.starts_with(folder))
            .map(|(_, volume)| *volume)
    }

    fn fake_file_id(&self, name: &OsStr) -> Option<FileId> {
        let name = name.to_string_lossy();
        self.file_ids
            .lock()
            .unwrap()
            .iter()
            .find(|(wanted, _)| *wanted == name)
            .map(|(_, id)| *id)
    }

    fn adjust(&self, path: &Path, stat: &mut Stat) {
        if let Some(volume) = self.fake_volume(path) {
            stat.volume = volume;
            // Another volume cannot have the same File ID in any meaningful way.
            stat.file_id = stat.file_id.map(|id| scrambled(id, volume));
        }
        if let Some(name) = path.file_name()
            && let Some(id) = self.fake_file_id(name)
        {
            stat.file_id = Some(id);
        }
    }
}

fn scrambled(id: FileId, volume: u64) -> FileId {
    let mut bytes = id.0;
    for (byte, mask) in bytes.iter_mut().zip(volume.to_le_bytes().iter().cycle()) {
        *byte ^= mask;
    }
    FileId(bytes)
}

impl FsProbe for RedirectingProbe {
    fn list_directory(&self, path: &Path) -> io::Result<Listing> {
        let mut listing = RealFileSystem.list_directory(path)?;
        if let Some(volume) = self.fake_volume(path) {
            listing.volume = volume;
            listing.directory_id = listing.directory_id.map(|id| scrambled(id, volume));
            for entry in &mut listing.entries {
                entry.file_id = entry.file_id.map(|id| scrambled(id, volume));
            }
        }
        for entry in &mut listing.entries {
            if let Some(id) = self.fake_file_id(&entry.name) {
                entry.file_id = Some(id);
            }
        }
        Ok(listing)
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let mut stat = RealFileSystem.stat(path)?;
        self.adjust(path, &mut stat);
        Ok(stat)
    }

    fn open_content(&self, path: &Path, pattern: ReadPattern) -> Result<ContentFile, OpenError> {
        let mut content = RealFileSystem.open_content(path, pattern)?;
        self.adjust(path, &mut content.stat);
        Ok(content)
    }
}

/// A fixture whose probe is a [`RedirectingProbe`], which the test can reach.
pub fn redirecting_fixture() -> (Fixture, Arc<RedirectingProbe>) {
    let probe = Arc::new(RedirectingProbe::default());
    let fixture = Fixture::with_options(LibraryOptions {
        probe: probe.clone(),
        ..options()
    });
    (fixture, probe)
}
