//! Path B's WebView2 shared buffers.
//!
//! The slab itself lives in an `ICoreWebView2SharedBuffer`, whose section handle is
//! duplicated into the render workers. PDFium therefore writes into the very memory the
//! page's main thread reads, with no copy in between. A second buffer holds the delivery
//! ring, which render workers never map.
//!
//! Each buffer is posted to the page once and stays mapped there. Posting per tile would
//! mean a mapping and a UI-thread hop for every tile. These COM objects belong to the UI
//! thread, so they live in a thread-local and every call on them happens there.
//!
//! The probe buffers are separate and much smaller. The page tests them before any session
//! exists, to decide which delivery path to ask for.

use std::cell::RefCell;
use std::ptr::{self, NonNull};
use std::sync::{Mutex, PoisonError, mpsc};

use musaeum_shm::ring::ring_len;
use musaeum_shm::slab::slab_len;
use musaeum_shm::{DeliveryRing, PixelSlab, ShmRegion};
use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_SHARED_BUFFER_ACCESS, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
    COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE, ICoreWebView2_17, ICoreWebView2Environment12,
    ICoreWebView2SharedBuffer,
};
use windows::Win32::Foundation::HANDLE;
use windows::core::{Interface, PCWSTR};

/// `"MPRB"`, marking the small buffers that test what JavaScript may do with these
/// buffers. See `apps/desktop/src/probe.ts`.
const PROBE_MAGIC: u32 = u32::from_le_bytes(*b"MPRB");
const PROBE_LEN: usize = 4096;
/// Word of a probe buffer that the host writes and a script reads.
const PROBE_HOST_WORD: usize = 1;
/// Word of a probe buffer that a script writes and the host reads.
const PROBE_SCRIPT_WORD: usize = 2;

thread_local! {
    /// Buffers alive on the UI thread, closed by [`release_buffers`].
    static LIVE_BUFFERS: RefCell<Vec<ICoreWebView2SharedBuffer>> = const { RefCell::new(Vec::new()) };
}

/// The host's own view of the probe buffers.
static PROBES: Mutex<Vec<ShmRegion>> = Mutex::new(Vec::new());

/// Writes `value` into every probe buffer and returns what scripts wrote into them.
pub fn probe_exchange(value: u32) -> Vec<u32> {
    let probes = PROBES.lock().unwrap_or_else(PoisonError::into_inner);
    probes
        .iter()
        .map(|region| {
            let words = region.as_ptr().cast::<u32>();
            // SAFETY: each probe is `PROBE_LEN` bytes and page-aligned, so both words are
            // in bounds. Volatile, because a script in another process may be using them.
            // The regions stay valid because `release_buffers` empties this list first.
            unsafe {
                words.add(PROBE_HOST_WORD).write_volatile(value);
                words.add(PROBE_SCRIPT_WORD).read_volatile()
            }
        })
        .collect()
}

/// The shared memory Path B runs on.
pub struct RingBuffers {
    /// The pixel slab, visible read-only to the page.
    pub slab: PixelSlab,
    /// The delivery ring, visible read-write to the page.
    pub ring: DeliveryRing,
}

/// Runs `work` on the UI thread with the interfaces shared buffers need, and waits for its
/// answer. Must not be called from the UI thread.
fn on_ui_thread<T: Send + 'static>(
    window: &WebviewWindow,
    work: impl FnOnce(&ICoreWebView2_17, &ICoreWebView2Environment12) -> Result<T, String>
    + Send
    + 'static,
) -> Result<T, String> {
    let (sender, receiver) = mpsc::channel();
    window
        .with_webview(move |webview| {
            let environment = webview.environment();
            let controller = webview.controller();
            // SAFETY: this closure runs on the UI thread, where these interfaces live.
            let core = unsafe { controller.CoreWebView2() };
            let result = core
                .map_err(|error| format!("getting the WebView: {error}"))
                .and_then(|core| {
                    work(
                        &core.cast().map_err(interface_error)?,
                        &environment.cast().map_err(interface_error)?,
                    )
                });
            let _ = sender.send(result);
        })
        .map_err(|error| format!("reaching the WebView: {error}"))?;
    receiver
        .recv()
        .map_err(|_| "the UI thread dropped the request".to_owned())?
}

fn interface_error(error: windows::core::Error) -> String {
    format!("this WebView2 runtime lacks shared buffers: {error}")
}

/// Creates the session's buffers and posts them to the page. Blocks until the UI thread has
/// done that, so it must not be called from the UI thread.
pub fn create_ring_buffers(
    window: &WebviewWindow,
    slots: u32,
    ring_capacity: u32,
) -> Result<RingBuffers, String> {
    on_ui_thread(window, move |core, environment| {
        // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
        let (slab_buffer, slab_region) = unsafe { create_buffer(environment, slab_len(slots)) }?;
        let slab = PixelSlab::init(slab_region, slots).map_err(|error| error.to_string())?;

        // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
        let (ring_buffer, ring_region) =
            unsafe { create_buffer(environment, ring_len(ring_capacity)) }?;
        let ring =
            DeliveryRing::init(ring_region, ring_capacity).map_err(|error| error.to_string())?;

        // Both are formatted before the page can see them. The page tells them apart by the
        // magic number in their first word, so no JSON travels with them.
        // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
        unsafe {
            post(
                core,
                ring_buffer,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
            )?;
            post(
                core,
                slab_buffer,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
            )?;
        }
        Ok(RingBuffers { slab, ring })
    })
}

/// Creates two small buffers for the page to test, one posted read-only and one read-write,
/// the way the session's slab and ring would be. A failure here means this runtime cannot
/// do shared buffers at all. Must not be called from the UI thread.
pub fn create_probe_buffers(window: &WebviewWindow) -> Result<(), String> {
    on_ui_thread(window, |core, environment| {
        let accesses = [
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
        ];
        for access in accesses {
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            let (buffer, region) = unsafe { create_buffer(environment, PROBE_LEN) }?;
            // SAFETY: the buffer is at least 4 bytes, page-aligned, and not shared yet.
            unsafe { region.as_ptr().cast::<u32>().write(PROBE_MAGIC) };
            PROBES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(region);
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            unsafe { post(core, buffer, access) }?;
        }
        Ok(())
    })
}

/// Posts a buffer to the page and keeps it alive until [`release_buffers`].
///
/// # Safety
///
/// Must run on the WebView's UI thread.
unsafe fn post(
    core: &ICoreWebView2_17,
    buffer: ICoreWebView2SharedBuffer,
    access: COREWEBVIEW2_SHARED_BUFFER_ACCESS,
) -> Result<(), String> {
    // Tracked first, so that a failure below still gets the buffer closed.
    LIVE_BUFFERS.with_borrow_mut(|live| live.push(buffer.clone()));
    // SAFETY: on the UI thread, per this function's contract; a null string means no
    // additional data.
    unsafe { core.PostSharedBufferToScript(&buffer, access, PCWSTR::null()) }
        .map_err(|error| format!("posting a shared buffer: {error}"))
}

/// # Safety
///
/// Must run on the WebView's UI thread.
unsafe fn create_buffer(
    environment: &ICoreWebView2Environment12,
    len: usize,
) -> Result<(ICoreWebView2SharedBuffer, ShmRegion), String> {
    // SAFETY: on the UI thread, per this function's contract.
    let buffer = unsafe { environment.CreateSharedBuffer(len as u64) }
        .map_err(|error| format!("creating a shared buffer of {len} bytes: {error}"))?;

    let mut pointer = ptr::null_mut();
    let mut section = HANDLE::default();
    // SAFETY: both out-pointers are valid for the duration of the calls.
    unsafe {
        buffer
            .Buffer(&mut pointer)
            .map_err(|error| error.to_string())?;
        buffer
            .FileMappingHandle(&mut section)
            .map_err(|error| error.to_string())?;
    }
    let pointer = NonNull::new(pointer).ok_or("WebView2 returned a null buffer")?;

    // SAFETY: the mapping and the handle stay valid until `Close`, which only
    // `release_buffers` calls, and only after the pool using them has stopped.
    let region = unsafe { ShmRegion::from_external(pointer, len, section.0) };
    Ok((buffer, region))
}

/// Closes the buffers on the UI thread. The session's pool must already have stopped.
/// Failing to reach the UI thread means the app is exiting, when it no longer matters.
pub fn release_buffers(window: &WebviewWindow) {
    PROBES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let _ = window.with_webview(|_| {
        LIVE_BUFFERS.with_borrow_mut(|live| {
            for buffer in live.drain(..) {
                // SAFETY: on the UI thread, and the only users of the memory have stopped.
                if let Err(error) = unsafe { buffer.Close() } {
                    eprintln!("closing a shared buffer: {error}");
                }
            }
        });
    });
}
