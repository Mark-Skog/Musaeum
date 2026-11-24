//! Drives a real worker binary over the real protocol and shared memory, inside the sandbox the app
//! runs it in. For the worker crates' tests, so each format is tested the same way.

use std::fs::File;
use std::io::{BufReader, Write};
use std::os::windows::io::AsHandle;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use musaeum_core::{DeliveryMode, FixedFormat, PoolConfig, PoolStats, RenderPool, encode_demand};

use musaeum_sandbox::{Documents, Policy, SandboxedChild, SpawnConfig};
use musaeum_shm::region::duplicate_file_read_only;
use musaeum_shm::slab::slab_len;
use musaeum_shm::wire::{
    AnalyzeJob, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, TextJob, WorkerMessage,
    read_frame, write_frame,
};
use musaeum_shm::{
    CharLayer, PageGeometry, PixelSlab, ShmRegion, TILE_BYTES, TILE_SIZE, TextLayer,
    TileAnnouncement, TileKey,
};

/// A worker process and the host's side of its pipes and slab.
pub struct Worker {
    /// The process.
    pub child: SandboxedChild,
    stdin: Option<File>,
    stdout: BufReader<File>,
    /// The slab it renders into.
    pub slab: PixelSlab,
    _document: File,
}

impl Worker {
    /// Starts `exe` on `document` with a slab of `slots` slots, and sends the first message.
    pub fn spawn(
        exe: &Path,
        document: &Path,
        policy: Policy,
        slots: u32,
        send_geometry: bool,
        protocol: u32,
    ) -> Self {
        Self::spawn_limited(exe, document, policy, slots, send_geometry, protocol, None)
    }

    fn spawn_limited(
        exe: &Path,
        document: &Path,
        policy: Policy,
        slots: u32,
        send_geometry: bool,
        protocol: u32,
        memory_limit: Option<usize>,
    ) -> Self {
        let mut child = musaeum_sandbox::spawn(SpawnConfig {
            exe,
            args: &[],
            policy,
            memory_limit,
        })
        .expect("the worker starts");
        let mut stdin = child.stdin.take().expect("a stdin pipe");
        let stdout = BufReader::new(child.stdout.take().expect("a stdout pipe"));

        let region = ShmRegion::create(slab_len(slots)).expect("a slab");
        let slab = PixelSlab::init(region, slots).expect("a formatted slab");
        let file = File::open(document).expect("the document opens");
        let init = HostMessage::Init(InitParams {
            protocol,
            worker_index: 0,
            send_geometry,
            slab_handle: slab
                .region()
                .duplicate_into(child.as_handle())
                .expect("the slab is shared"),
            slab_len: slab.region().len() as u64,
            doc_handle: duplicate_file_read_only(&file, child.as_handle())
                .expect("the document is shared"),
        });
        write_frame(&mut stdin, &init.encode()).expect("the first message goes out");
        stdin.flush().expect("the first message goes out");

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            slab,
            _document: file,
        }
    }

    /// Starts `exe` on `document` and waits for it to report ready, without geometry.
    pub fn started(exe: &Path, document: &Path, policy: Policy) -> Self {
        let mut worker = Self::spawn(exe, document, policy, 4, false, PROTOCOL_VERSION);
        match worker.receive() {
            Some(WorkerMessage::Ready { .. }) => worker,
            other => panic!("the worker did not start: {other:?}"),
        }
    }

    /// Starts `exe` on `document` as the app's reader starts the worker of `format`: under the
    /// policy the app gives that format, with its memory capped, and waits for it to report ready.
    pub fn started_as_in_the_app(exe: &Path, document: &Path, format: FixedFormat) -> Self {
        let mut worker = Self::spawn_limited(
            exe,
            document,
            format.policy(),
            4,
            false,
            PROTOCOL_VERSION,
            Some(1 << 30),
        );
        match worker.receive() {
            Some(WorkerMessage::Ready { .. }) => worker,
            other => panic!("the worker did not start: {other:?}"),
        }
    }

    /// Everything about this worker that falls short of `policy`, read from outside as the kernel
    /// reports it: the job, the startup token, the token, the mitigations and every handle it
    /// holds, where the only file may be `document`, read only. Prints what it read.
    pub fn sandbox_violations(&self, policy: Policy, document: &Path) -> Vec<String> {
        let report = self
            .child
            .report()
            .expect("the worker's sandbox can be read");
        let audit = self
            .child
            .audit()
            .expect("the worker's handles can be read");
        println!("{report:#?}\n{audit:#?}");
        let name = document
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a document with a name");
        let mut found = report.violations(policy);
        found.extend(audit.violations(policy, Documents::Only(&[name])));
        found
    }

    /// Starts the worker asking for the geometry, and collects it: ready, page sizes, hints.
    pub fn with_geometry(
        exe: &Path,
        document: &Path,
        policy: Policy,
    ) -> (Self, Vec<PageGeometry>, (u8, u8)) {
        let mut worker = Self::spawn(exe, document, policy, 4, true, PROTOCOL_VERSION);
        let page_count = match worker.receive() {
            Some(WorkerMessage::Ready { page_count, .. }) => page_count,
            other => panic!("the worker did not start: {other:?}"),
        };
        let mut pages = Vec::new();
        let mut hints = (0, 0);
        loop {
            match worker.receive() {
                Some(WorkerMessage::Geometry { page, size }) => {
                    assert_eq!(page as usize, pages.len(), "pages arrive in order");
                    pages.push(size);
                }
                Some(WorkerMessage::Hints {
                    page_layout,
                    direction,
                }) => hints = (page_layout, direction),
                Some(WorkerMessage::GeometryEnd {
                    page_count: sent, ..
                }) => {
                    assert_eq!(sent, page_count);
                    assert_eq!(pages.len(), page_count as usize);
                    return (worker, pages, hints);
                }
                other => panic!("unexpected message during the geometry: {other:?}"),
            }
        }
    }

    /// The next message, or `None` once the worker has gone.
    pub fn receive(&mut self) -> Option<WorkerMessage> {
        read_frame(&mut self.stdout)
            .expect("the pipe stays readable")
            .map(|frame| WorkerMessage::decode(&frame).expect("a well-formed message"))
    }

    /// Sends one message.
    pub fn send(&mut self, message: HostMessage) {
        let stdin = self.stdin.as_mut().expect("the pipe is open");
        write_frame(stdin, &message.encode()).expect("the message goes out");
        stdin.flush().expect("the message goes out");
    }

    /// Closes the host's end of the pipe, as a host that crashed would.
    pub fn close_pipe(&mut self) {
        self.stdin = None;
    }

    /// Renders one tile and waits for the answer.
    pub fn request(&mut self, job_seq: u64, slot: u32, tile: TileKey) -> WorkerMessage {
        self.send(HostMessage::Render(RenderJob {
            job_seq,
            slot,
            tile,
            background: false,
        }));
        self.receive().expect("the worker should answer")
    }

    /// Classifies one page and waits for the answer.
    pub fn analyze(&mut self, job_seq: u64, page: u32) -> WorkerMessage {
        self.send(HostMessage::Analyze(AnalyzeJob { job_seq, page }));
        self.receive().expect("the worker should answer")
    }

    /// Reads one page's text layer through `slot`, and decodes it.
    pub fn text(&mut self, job_seq: u64, slot: u32, page: u32) -> Result<TextLayer, WorkerMessage> {
        self.send(HostMessage::ExtractText(TextJob {
            job_seq,
            slot,
            page,
        }));
        match self.receive().expect("the worker should answer") {
            WorkerMessage::TextDone { len, .. } => {
                let bytes = self
                    .slab
                    .copy_slot_prefix(slot, len as usize)
                    .expect("a valid slot");
                Ok(TextLayer::decode(&bytes).expect("a well-formed text layer"))
            }
            other => Err(other),
        }
    }

    /// Reads one page's characters through `slot`, and decodes them.
    pub fn chars(
        &mut self,
        job_seq: u64,
        slot: u32,
        page: u32,
    ) -> Result<CharLayer, WorkerMessage> {
        self.send(HostMessage::ExtractChars(TextJob {
            job_seq,
            slot,
            page,
        }));
        match self.receive().expect("the worker should answer") {
            WorkerMessage::TextDone { len, .. } => {
                let bytes = self
                    .slab
                    .copy_slot_prefix(slot, len as usize)
                    .expect("a valid slot");
                Ok(CharLayer::decode(&bytes).expect("a well-formed character layer"))
            }
            other => Err(other),
        }
    }

    /// One pixel of a finished tile, as RGBA.
    pub fn pixel(&self, slot: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * TILE_SIZE + x) * 4) as usize;
        assert!(offset + 4 <= TILE_BYTES);
        // SAFETY: the offset is inside the slot, and the worker has reported the job as
        // finished, so nothing is writing to this slot any more.
        unsafe {
            self.slab
                .slot_ptr(slot)
                .expect("a valid slot")
                .add(offset)
                .cast::<[u8; 4]>()
                .read()
        }
    }

    /// The `width` × `height` pixels at a finished tile's top-left corner, rows packed.
    pub fn rect(&self, slot: u32, width: u32, height: u32) -> Vec<u8> {
        self.slab
            .copy_slot_rect(slot, width, height)
            .expect("a valid slot")
    }

    /// Whether a slot still holds nothing but zeroes.
    pub fn slot_untouched(&self, slot: u32) -> bool {
        self.slab
            .copy_slot_prefix(slot, TILE_BYTES)
            .expect("a valid slot")
            .iter()
            .all(|&byte| byte == 0)
    }
}

/// How long the pool helpers wait for anything.
pub const WAIT: Duration = Duration::from_secs(60);

/// A finished tile as a pool delivered it.
pub struct PulledTile {
    /// What the pool said about it.
    pub tile: TileAnnouncement,
    /// Its pixels: a whole tile, RGBA.
    pub pixels: Vec<u8>,
}

impl PulledTile {
    /// One pixel, as RGBA.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * TILE_SIZE + x) * 4) as usize;
        self.pixels[at..at + 4].try_into().expect("four bytes")
    }
}

/// Starts a pool of `workers` processes of `exe` reading `document` under `policy`, and sends
/// every tile it delivers to the returned channel.
pub fn pull_pool(
    exe: &Path,
    document: &Path,
    policy: Policy,
    workers: usize,
    slots: u32,
) -> (Arc<RenderPool>, Receiver<PulledTile>) {
    let slab = PixelSlab::init(ShmRegion::create(slab_len(slots)).expect("a slab"), slots)
        .expect("a formatted slab");
    let pool = RenderPool::start(
        PoolConfig {
            worker_exe: exe.to_path_buf(),
            document: document.to_path_buf(),
            worker_count: workers,
            process_memory_limit: Some(2 << 30),
            policy,
            startup_timeout: WAIT,
            known_geometry: None,
        },
        Arc::new(slab),
        DeliveryMode::Pull { max_batch: 4 },
    )
    .expect("the pool starts");
    let pool = Arc::new(pool);
    let (sender, receiver) = mpsc::channel();
    // Several parked pollers, like the compositor's concurrent fetches.
    for _ in 0..3 {
        park(Arc::clone(&pool), sender.clone());
    }
    (pool, receiver)
}

fn park(pool: Arc<RenderPool>, sender: mpsc::Sender<PulledTile>) {
    let again = Arc::clone(&pool);
    pool.park_poller(Box::new(move |batch| {
        if batch.is_empty() {
            return;
        }
        for tile in parse_batch(&batch) {
            let _ = sender.send(tile);
        }
        park(again, sender);
    }));
}

fn parse_batch(batch: &[u8]) -> Vec<PulledTile> {
    const WORDS: usize = 32;
    let word = |at: usize| u32::from_le_bytes(batch[at..at + 4].try_into().expect("four bytes"));
    assert_eq!(word(0), musaeum_core::pool::BATCH_MAGIC);
    let count = word(4) as usize;
    assert_eq!(batch.len(), 8 + count * (WORDS + TILE_BYTES));
    (0..count)
        .map(|index| {
            let at = 8 + index * (WORDS + TILE_BYTES);
            let words: [u32; 8] = std::array::from_fn(|n| word(at + n * 4));
            PulledTile {
                tile: TileAnnouncement::from_words(words),
                pixels: batch[at + WORDS..at + WORDS + TILE_BYTES].to_vec(),
            }
        })
        .collect()
}

/// Waits for a tile of `page`, dropping any other that arrives first.
pub fn wait_for_page(tiles: &Receiver<PulledTile>, page: u32) -> PulledTile {
    loop {
        let tile = tiles.recv_timeout(WAIT).expect("the tile should arrive");
        if tile.tile.page == page {
            return tile;
        }
    }
}

/// Ends a process the way the task manager does, with no chance to clean up.
pub fn terminate(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: plain calls with no pointers; the handle is checked and closed.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!process.is_null(), "could not open worker {pid}");
        assert_ne!(TerminateProcess(process, 0xDEAD), 0);
        CloseHandle(process);
    }
}

/// What killing a pool's worker mid-render cost.
pub struct Recovery {
    /// The pool's measurements afterwards, with `recovery_us` among them.
    pub stats: PoolStats,
    /// From each kill to the next tile a reader asked for, sorted.
    pub until_next_tile: Vec<Duration>,
}

/// Kills the pool's only worker `kills` times, each time while it renders `heavy(round)`, then asks
/// for `next(round)` as a reader turning the page would, and waits for it.
pub fn kill_and_recover(
    pool: &RenderPool,
    tiles: &Receiver<PulledTile>,
    kills: usize,
    heavy: impl Fn(usize) -> TileKey,
    next: impl Fn(usize) -> TileKey,
) -> Recovery {
    let mut until_next_tile = Vec::with_capacity(kills);
    // Far above any epoch a test used before calling this.
    let mut epoch = 1_000_000;
    for round in 0..kills {
        let pid = pool.worker_pids()[0].expect("the worker is running");
        epoch += 1;
        pool.submit_demand(encode_demand(epoch, &[heavy(round)]));
        std::thread::sleep(Duration::from_millis(3));
        let killed_at = Instant::now();
        terminate(pid);

        let wanted = next(round);
        epoch += 1;
        pool.submit_demand(encode_demand(epoch, &[wanted]));
        wait_for_page(tiles, wanted.page);
        until_next_tile.push(killed_at.elapsed());
        let replacement = pool.worker_pids()[0];
        assert!(replacement.is_some_and(|new| new != pid), "a new process");
    }
    until_next_tile.sort();
    Recovery {
        stats: pool.stats().expect("the pool is running"),
        until_next_tile,
    }
}
