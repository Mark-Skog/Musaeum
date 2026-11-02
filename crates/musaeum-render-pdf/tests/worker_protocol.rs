//! Drives the real worker binary with the real PDFium over real shared memory, inside the
//! same sandbox the app runs it in.

use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};
use musaeum_shm::region::duplicate_file_read_only;
use musaeum_shm::slab::slab_len;
use musaeum_shm::wire::{
    FatalCode, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, TileFailure, WorkerMessage,
    read_frame, write_frame,
};
use musaeum_shm::{PageGeometry, PixelSlab, ShmRegion, TILE_BYTES, TILE_SIZE, TileKey};

const SLOTS: u32 = 4;

fn temp_dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("worker-{test}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn mixed_pdf(test: &str) -> PathBuf {
    let path = temp_dir(test).join("mixed.pdf");
    gen_fixtures::write_mixed_fixture(&path).unwrap();
    path
}

struct Worker {
    child: SandboxedChild,
    stdin: Option<File>,
    stdout: BufReader<File>,
    slab: PixelSlab,
    _document: File,
}

impl Worker {
    fn spawn(pdf: &Path, send_geometry: bool, protocol: u32) -> Self {
        let mut child = musaeum_sandbox::spawn(SpawnConfig {
            exe: Path::new(env!("CARGO_BIN_EXE_musaeum-render-pdf")),
            args: &[],
            policy: Policy::Lockdown,
            memory_limit: None,
        })
        .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        let region = ShmRegion::create(slab_len(SLOTS)).unwrap();
        let slab = PixelSlab::init(region, SLOTS).unwrap();
        let document = File::open(pdf).unwrap();
        let init = HostMessage::Init(InitParams {
            protocol,
            worker_index: 0,
            send_geometry,
            slab_handle: slab.region().duplicate_into(child.as_handle()).unwrap(),
            slab_len: slab.region().len() as u64,
            doc_handle: duplicate_file_read_only(&document, child.as_handle()).unwrap(),
        });
        write_frame(&mut stdin, &init.encode()).unwrap();
        stdin.flush().unwrap();

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            slab,
            _document: document,
        }
    }

    fn started(pdf: &Path) -> Self {
        let mut worker = Self::spawn(pdf, false, PROTOCOL_VERSION);
        assert!(matches!(
            worker.receive(),
            Some(WorkerMessage::Ready { .. })
        ));
        worker
    }

    fn receive(&mut self) -> Option<WorkerMessage> {
        read_frame(&mut self.stdout)
            .unwrap()
            .map(|frame| WorkerMessage::decode(&frame).unwrap())
    }

    fn send(&mut self, message: HostMessage) {
        let stdin = self.stdin.as_mut().unwrap();
        write_frame(stdin, &message.encode()).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, job_seq: u64, slot: u32, tile: TileKey) -> WorkerMessage {
        self.send(HostMessage::Render(RenderJob {
            job_seq,
            slot,
            tile,
        }));
        self.receive().expect("the worker should answer")
    }

    fn pixel(&self, slot: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * TILE_SIZE + x) * 4) as usize;
        assert!(offset + 4 <= TILE_BYTES);
        // SAFETY: the offset is inside the slot, and the worker has reported the job as
        // finished, so nothing is writing to this slot any more.
        unsafe {
            self.slab
                .slot_ptr(slot)
                .unwrap()
                .add(offset)
                .cast::<[u8; 4]>()
                .read()
        }
    }

    fn slot_untouched(&self, slot: u32) -> bool {
        let start = self.slab.slot_ptr(slot).unwrap();
        // SAFETY: the whole slot is mapped, and no job is using it.
        unsafe { std::slice::from_raw_parts(start, TILE_BYTES) }
            .iter()
            .all(|&byte| byte == 0)
    }
}

fn tile(page: u32, lod: i8, x: u16, y: u16) -> TileKey {
    TileKey { page, lod, x, y }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn reports_real_geometry_for_every_page_without_rendering() {
    let pdf = mixed_pdf("geometry");
    let mut worker = Worker::spawn(&pdf, true, PROTOCOL_VERSION);

    let Some(WorkerMessage::Ready {
        protocol,
        page_count,
        ..
    }) = worker.receive()
    else {
        panic!("expected Ready first");
    };
    assert_eq!(protocol, PROTOCOL_VERSION);
    assert_eq!(page_count, 6);

    let expected = [
        (612.0, 792.0),   // portrait
        (842.0, 595.0),   // landscape
        (500.0, 500.0),   // square
        (792.0, 612.0),   // portrait MediaBox with /Rotate 90
        (300.0, 400.0),   // CropBox wins over the MediaBox
        (2000.0, 3000.0), // spans many tiles
    ];
    for (index, (width_pt, height_pt)) in expected.into_iter().enumerate() {
        assert_eq!(
            worker.receive(),
            Some(WorkerMessage::Geometry {
                page: index as u32,
                size: PageGeometry {
                    width_pt,
                    height_pt
                },
            })
        );
    }
    assert!(matches!(
        worker.receive(),
        Some(WorkerMessage::GeometryEnd { page_count: 6, .. })
    ));
    for slot in 0..SLOTS {
        assert!(
            worker.slot_untouched(slot),
            "reading geometry rendered something"
        );
    }
}

#[test]
fn writes_opaque_rgba_into_exactly_the_requested_slot() {
    let pdf = mixed_pdf("rgba");
    let mut worker = Worker::started(&pdf);

    let report = worker.request(1, 2, tile(0, 0, 0, 0));
    assert!(matches!(
        report,
        WorkerMessage::TileDone {
            job_seq: 1,
            slot: 2,
            ..
        }
    ));

    // Pure red, so this fails if the channels come out as BGRA.
    assert_eq!(worker.pixel(2, 256, 256), RED);
    assert!(worker.slot_untouched(0));
    assert!(worker.slot_untouched(1));
    assert!(worker.slot_untouched(3));
}

#[test]
fn every_channel_lands_where_it_should() {
    let pdf = mixed_pdf("channels");
    let mut worker = Worker::started(&pdf);

    worker.request(1, 0, tile(1, 0, 0, 0));
    assert_eq!(worker.pixel(0, 100, 100), GREEN);

    worker.request(2, 0, tile(2, 0, 0, 0));
    assert_eq!(worker.pixel(0, 100, 100), BLUE);

    // Yellow page behind /Rotate 90, and cyan page behind a CropBox.
    worker.request(3, 0, tile(3, 0, 0, 0));
    assert_eq!(worker.pixel(0, 100, 100), [255, 255, 0, 255]);
    worker.request(4, 0, tile(4, 0, 0, 0));
    assert_eq!(worker.pixel(0, 100, 100), [0, 255, 255, 255]);
}

#[test]
fn tiles_address_the_right_part_of_the_page() {
    let pdf = mixed_pdf("offsets");
    let mut worker = Worker::started(&pdf);

    // The 2000 × 3000 page: top-left quadrant red, bottom-right blue, the rest white.
    worker.request(1, 0, tile(5, 0, 0, 0));
    assert_eq!(worker.pixel(0, 256, 256), RED);

    worker.request(2, 0, tile(5, 0, 3, 0));
    assert_eq!(worker.pixel(0, 256, 256), WHITE);

    // Last tile of the grid: covers x 1536–2000, y 2560–3000.
    worker.request(3, 0, tile(5, 0, 3, 5));
    assert_eq!(worker.pixel(0, 100, 100), BLUE);
    // Past the page's edge the tile keeps the paper colour.
    assert_eq!(worker.pixel(0, 500, 500), WHITE);

    // The boundary between quadrants falls at x = 1000, inside the tile covering 512–1024.
    worker.request(4, 0, tile(5, 0, 1, 0));
    assert_eq!(worker.pixel(0, 1000 - 512 - 2, 100), RED);
    assert_eq!(worker.pixel(0, 1000 - 512 + 2, 100), WHITE);
}

#[test]
fn levels_of_detail_scale_the_page() {
    let pdf = mixed_pdf("lods");
    let mut worker = Worker::started(&pdf);

    // At 1/4 px/pt the 2000 × 3000 page is 500 × 750 pixels, so two tiles tall.
    worker.request(1, 0, tile(5, -2, 0, 0));
    assert_eq!(worker.pixel(0, 100, 100), RED);
    assert_eq!(
        worker.pixel(0, 400, 100),
        WHITE,
        "x = 1600 pt is above the blue quadrant"
    );

    worker.request(2, 0, tile(5, -2, 0, 1));
    assert_eq!(worker.pixel(0, 400, 100), BLUE, "x = 1600 pt, y = 2448 pt");
    assert_eq!(
        worker.pixel(0, 505, 100),
        WHITE,
        "past the page's right edge"
    );
}

#[test]
fn a_cancelled_job_is_reported_without_touching_the_slot() {
    let pdf = mixed_pdf("pre-cancel");
    let mut worker = Worker::started(&pdf);

    worker
        .slab
        .cancel_cell(0)
        .unwrap()
        .store(7, Ordering::Release);
    assert!(matches!(
        worker.request(7, 1, tile(0, 0, 0, 0)),
        WorkerMessage::TileCancelled {
            job_seq: 7,
            slot: 1,
            ..
        }
    ));
    assert!(worker.slot_untouched(1));

    // Cancelling job 7 must not affect job 8.
    assert!(matches!(
        worker.request(8, 1, tile(0, 0, 0, 0)),
        WorkerMessage::TileDone { job_seq: 8, .. }
    ));
}

#[test]
fn cancellation_interrupts_a_render_already_in_progress() {
    // Page 49 of the vector fixture is the map-like page with 15 000 paths.
    let pdf = temp_dir("mid-cancel").join("vector.pdf");
    gen_fixtures::write_vector_fixture(&pdf, 50).unwrap();
    let heavy = tile(49, 2, 0, 0);
    let mut worker = Worker::started(&pdf);

    // Load the page first, so the time below is purely rendering.
    worker.request(1, 0, heavy);
    let started = Instant::now();
    assert!(matches!(
        worker.request(2, 0, heavy),
        WorkerMessage::TileDone { .. }
    ));
    let full_render = started.elapsed();

    // Cancel a quarter of the way through. Having spent at least part of that time on the
    // job proves it had started, and stopping well before the end proves it was
    // interrupted. Several attempts, because the OS may delay either side.
    let delay = full_render / 4;
    let mut job_seq = 10;
    let mut interrupted = None;
    for _ in 0..20 {
        job_seq += 1;
        worker.send(HostMessage::Render(RenderJob {
            job_seq,
            slot: 0,
            tile: heavy,
        }));
        std::thread::sleep(delay);
        worker
            .slab
            .cancel_cell(0)
            .unwrap()
            .store(job_seq, Ordering::Release);
        if let Some(WorkerMessage::TileCancelled { elapsed_us, .. }) = worker.receive() {
            let elapsed = Duration::from_micros(u64::from(elapsed_us));
            if elapsed >= delay / 2 && elapsed < full_render {
                interrupted = Some(elapsed);
                break;
            }
        }
    }

    assert!(
        interrupted.is_some(),
        "no attempt interrupted a render that takes {full_render:?}"
    );
    // The worker is still healthy afterwards.
    assert!(matches!(
        worker.request(job_seq + 1, 0, heavy),
        WorkerMessage::TileDone { .. }
    ));
}

#[test]
fn bad_requests_fail_alone_and_the_worker_survives() {
    let pdf = mixed_pdf("bad-requests");
    let mut worker = Worker::started(&pdf);

    assert_eq!(
        worker.request(1, SLOTS, tile(0, 0, 0, 0)),
        WorkerMessage::TileFailed {
            job_seq: 1,
            slot: SLOTS,
            failure: TileFailure::InvalidSlot
        }
    );
    assert_eq!(
        worker.request(2, 0, tile(99, 0, 0, 0)),
        WorkerMessage::TileFailed {
            job_seq: 2,
            slot: 0,
            failure: TileFailure::OutOfBounds
        }
    );
    assert_eq!(
        worker.request(3, 0, tile(0, 0, 100, 0)),
        WorkerMessage::TileFailed {
            job_seq: 3,
            slot: 0,
            failure: TileFailure::OutOfBounds
        }
    );
    assert!(matches!(
        worker.request(4, 0, tile(0, 0, 0, 0)),
        WorkerMessage::TileDone { .. }
    ));
}

#[test]
fn page_cache_eviction_keeps_rendering_correctly() {
    let pdf = temp_dir("eviction").join("many.pdf");
    gen_fixtures::write_pdf(&pdf, &[], 20, |index| {
        let shade = index as f32 / 19.0;
        gen_fixtures::PageSpec::new(100.0, 100.0, format!("{shade} 0 0 rg 0 0 100 100 re f"))
    })
    .unwrap();
    let mut worker = Worker::started(&pdf);

    // Twice through more pages than the cache holds, so pages get evicted and reloaded.
    for round in 0..2u64 {
        for page in 0..20u32 {
            let seq = round * 100 + u64::from(page) + 1;
            assert!(matches!(
                worker.request(seq, 0, tile(page, 0, 0, 0)),
                WorkerMessage::TileDone { .. }
            ));
            let expected = (page as f32 / 19.0 * 255.0).round() as i16;
            let [red, ..] = worker.pixel(0, 50, 50);
            assert!(
                (i16::from(red) - expected).abs() <= 1,
                "page {page} rendered as if it were another page"
            );
        }
    }
}

#[test]
fn giving_memory_back_does_not_change_what_renders_or_how_it_answers() {
    let pdf = mixed_pdf("trim");
    let mut worker = Worker::started(&pdf);

    worker.request(1, 0, tile(0, 0, 0, 0));
    assert_eq!(worker.pixel(0, 256, 256), RED);

    // Nothing is sent back for this, so the next answer has to be for the next job.
    worker.send(HostMessage::Trim);
    let report = worker.request(2, 1, tile(1, 0, 0, 0));
    assert!(
        matches!(report, WorkerMessage::TileDone { job_seq: 2, .. }),
        "{report:?}"
    );
    assert_eq!(worker.pixel(1, 100, 100), GREEN);

    // And again on a page it had cached before the trim.
    worker.send(HostMessage::Trim);
    worker.send(HostMessage::Trim);
    worker.request(3, 2, tile(0, 0, 0, 0));
    assert_eq!(worker.pixel(2, 256, 256), RED);
}

#[test]
fn a_corrupt_pdf_is_a_reported_failure_not_a_crash() {
    let path = temp_dir("corrupt").join("corrupt.pdf");
    fs::write(&path, b"this is not a pdf at all").unwrap();
    let mut worker = Worker::spawn(&path, false, PROTOCOL_VERSION);
    assert_eq!(
        worker.receive(),
        Some(WorkerMessage::Fatal {
            code: FatalCode::DocumentOpen,
            detail: 3 // FPDF_ERR_FORMAT
        })
    );
}

#[test]
fn an_empty_file_is_a_reported_failure() {
    let path = temp_dir("empty").join("empty.pdf");
    fs::write(&path, b"").unwrap();
    let mut worker = Worker::spawn(&path, false, PROTOCOL_VERSION);
    assert!(matches!(
        worker.receive(),
        Some(WorkerMessage::Fatal {
            code: FatalCode::DocumentMap | FatalCode::DocumentOpen,
            ..
        })
    ));
}

#[test]
fn a_protocol_mismatch_is_refused() {
    let pdf = mixed_pdf("protocol");
    let mut worker = Worker::spawn(&pdf, false, PROTOCOL_VERSION + 1);
    assert_eq!(
        worker.receive(),
        Some(WorkerMessage::Fatal {
            code: FatalCode::ProtocolMismatch,
            detail: PROTOCOL_VERSION + 1
        })
    );
}

#[test]
fn exits_cleanly_when_asked_and_when_the_host_disappears() {
    let pdf = mixed_pdf("exit");

    let mut worker = Worker::started(&pdf);
    worker.send(HostMessage::Shutdown);
    assert_eq!(worker.child.wait().unwrap(), 0);

    let mut worker = Worker::started(&pdf);
    drop(worker.stdin.take());
    assert_eq!(worker.child.wait().unwrap(), 0);
}

#[test]
fn large_page_counts_open_quickly_because_nothing_is_rasterized() {
    let pdf = temp_dir("speed").join("long.pdf");
    gen_fixtures::write_pdf(&pdf, &[], 5000, |_| {
        gen_fixtures::PageSpec::new(612.0, 792.0, "0 0 1 rg 0 0 612 792 re f".into())
    })
    .unwrap();

    let mut worker = Worker::spawn(&pdf, true, PROTOCOL_VERSION);
    assert!(matches!(
        worker.receive(),
        Some(WorkerMessage::Ready {
            page_count: 5000,
            ..
        })
    ));
    let elapsed_us = loop {
        match worker.receive() {
            Some(WorkerMessage::GeometryEnd { elapsed_us, .. }) => break elapsed_us,
            Some(WorkerMessage::Geometry { .. }) => {}
            other => panic!("unexpected message: {other:?}"),
        }
    };
    assert!(
        elapsed_us < 2_000_000,
        "reading 5000 page sizes took {elapsed_us} µs"
    );
}
