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
use std::os::windows::io::BorrowedHandle;
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

/// Which owner posted a WebView2 shared buffer, so releasing one never closes another's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferScope {
    Probe,
    Session(u32),
}

thread_local! {
    /// Buffers alive on the UI thread, closed selectively by scope.
    static LIVE_BUFFERS: RefCell<Vec<(BufferScope, ICoreWebView2SharedBuffer)>> =
        const { RefCell::new(Vec::new()) };
}

/// The host's own view of the probe buffers.
static PROBES: Mutex<Vec<ShmRegion>> = Mutex::new(Vec::new());

/// Partitions `items` in place, returning the elements matching `wanted` (`None` matches all).
fn drain_matching_scope<T>(
    items: &mut Vec<(BufferScope, T)>,
    wanted: Option<BufferScope>,
) -> Vec<T> {
    let mut drained = Vec::new();
    let mut index = 0;
    while index < items.len() {
        if wanted.is_none_or(|scope| items[index].0 == scope) {
            drained.push(items.swap_remove(index).1);
        } else {
            index += 1;
        }
    }
    drained
}

/// Writes `value` into every probe buffer and returns what scripts wrote into them.
pub fn probe_exchange(value: u32) -> Vec<u32> {
    let probes = PROBES.lock().unwrap_or_else(PoisonError::into_inner);
    probes
        .iter()
        .map(|region| {
            let words = region.as_ptr().cast::<u32>();
            // SAFETY: each probe is `PROBE_LEN` bytes and page-aligned, so both words are
            // in bounds. Volatile, because a script in another process may be using them.
            // Each `ShmRegion` owns its own duplicated handle and mapped view, so closing
            // the `ICoreWebView2SharedBuffer` on the UI thread never unmaps `words`.
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
    session_id: u32,
    slots: u32,
    ring_capacity: u32,
) -> Result<RingBuffers, String> {
    let scope = BufferScope::Session(session_id);
    on_ui_thread(window, move |core, environment| {
        let created = (|| -> Result<RingBuffers, String> {
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            let (slab_buffer, slab_region) =
                unsafe { create_buffer(environment, slab_len(slots)) }?;
            let slab = match PixelSlab::init(slab_region, slots) {
                Ok(slab) => slab,
                Err(error) => {
                    // SAFETY: on the UI thread, closing the unposted buffer we just created.
                    let _ = unsafe { slab_buffer.Close() };
                    return Err(error.to_string());
                }
            };

            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            let (ring_buffer, ring_region) =
                match unsafe { create_buffer(environment, ring_len(ring_capacity)) } {
                    Ok(pair) => pair,
                    Err(error) => {
                        // SAFETY: on the UI thread, closing the unposted slab buffer on failure.
                        let _ = unsafe { slab_buffer.Close() };
                        return Err(error);
                    }
                };
            let ring = match DeliveryRing::init(ring_region, ring_capacity) {
                Ok(ring) => ring,
                Err(error) => {
                    // SAFETY: on the UI thread, closing both unposted buffers on failure.
                    unsafe {
                        let _ = ring_buffer.Close();
                        let _ = slab_buffer.Close();
                    }
                    return Err(error.to_string());
                }
            };

            // Both are formatted before the page can see them. The page tells them apart by the
            // magic number in their first word, so no JSON travels with them.
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            unsafe {
                if let Err(error) = post(
                    core,
                    scope,
                    ring_buffer,
                    COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
                ) {
                    let _ = slab_buffer.Close();
                    return Err(error);
                }
                post(
                    core,
                    scope,
                    slab_buffer,
                    COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
                )?;
            }
            Ok(RingBuffers { slab, ring })
        })();
        if created.is_err() {
            // SAFETY: on the UI thread; cleans up any buffer already pushed into `LIVE_BUFFERS`.
            unsafe { close_live_buffers(Some(scope)) };
        }
        created
    })
}

/// Creates two small buffers for the page to test, one posted read-only and one read-write,
/// the way the session's slab and ring would be. A failure here means this runtime cannot
/// do shared buffers at all. Must not be called from the UI thread.
pub fn create_probe_buffers(window: &WebviewWindow) -> Result<(), String> {
    on_ui_thread(window, |core, environment| {
        PROBES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        // SAFETY: on the UI thread; replaces any previous probe buffers without touching sessions.
        unsafe { close_live_buffers(Some(BufferScope::Probe)) };

        let accesses = [
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
        ];
        let mut regions = Vec::with_capacity(accesses.len());
        for access in accesses {
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            let (buffer, region) = match unsafe { create_buffer(environment, PROBE_LEN) } {
                Ok(pair) => pair,
                Err(error) => {
                    // SAFETY: on the UI thread; closes any probe buffer already posted in this loop.
                    unsafe { close_live_buffers(Some(BufferScope::Probe)) };
                    return Err(error);
                }
            };
            // SAFETY: the buffer is at least 4 bytes, page-aligned, and not shared yet.
            unsafe { region.as_ptr().cast::<u32>().write(PROBE_MAGIC) };
            regions.push(region);
            // SAFETY: on the UI thread, which is what `on_ui_thread` is for.
            if let Err(error) = unsafe { post(core, BufferScope::Probe, buffer, access) } {
                // SAFETY: on the UI thread; closes any probe buffers already tracked.
                unsafe { close_live_buffers(Some(BufferScope::Probe)) };
                return Err(error);
            }
        }
        *PROBES.lock().unwrap_or_else(PoisonError::into_inner) = regions;
        Ok(())
    })
}

/// Posts a buffer to the page and keeps it alive under `scope` until released.
///
/// # Safety
///
/// Must run on the WebView's UI thread.
unsafe fn post(
    core: &ICoreWebView2_17,
    scope: BufferScope,
    buffer: ICoreWebView2SharedBuffer,
    access: COREWEBVIEW2_SHARED_BUFFER_ACCESS,
) -> Result<(), String> {
    // Tracked first, so that a failure below still gets the buffer closed.
    LIVE_BUFFERS.with_borrow_mut(|live| live.push((scope, buffer.clone())));
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

    let mut section = HANDLE::default();
    // SAFETY: `section` is a valid out-pointer for the duration of the call.
    if let Err(error) = unsafe { buffer.FileMappingHandle(&mut section) } {
        // SAFETY: on the UI thread, closing the freshly created buffer on failure.
        let _ = unsafe { buffer.Close() };
        return Err(error.to_string());
    }
    if section.is_invalid() {
        // SAFETY: on the UI thread, closing the freshly created buffer on failure.
        let _ = unsafe { buffer.Close() };
        return Err("WebView2 returned an invalid shared-buffer handle".to_owned());
    }

    // SAFETY: `section` belongs to `buffer`, which is open on this thread for the duration of
    // `from_borrowed_section`. Duplicating the kernel handle and mapping our own view ensures
    // `buffer.Close()` can never unmap memory that Rust threads or pools still reference.
    let borrowed = unsafe { BorrowedHandle::borrow_raw(section.0) };
    match ShmRegion::from_borrowed_section(borrowed, len) {
        Ok(region) => Ok((buffer, region)),
        Err(error) => {
            // SAFETY: on the UI thread, closing the freshly created buffer on failure.
            let _ = unsafe { buffer.Close() };
            Err(format!("mapping a shared buffer of {len} bytes: {error}"))
        }
    }
}

/// Closes buffers matching `wanted` (`None` closes every tracked buffer).
///
/// # Safety
///
/// Must run on the WebView's UI thread.
unsafe fn close_live_buffers(wanted: Option<BufferScope>) {
    let buffers = LIVE_BUFFERS.with_borrow_mut(|live| drain_matching_scope(live, wanted));
    for buffer in buffers {
        // SAFETY: on the UI thread; host `ShmRegion`s hold their own duplicated handles and views.
        if let Err(error) = unsafe { buffer.Close() } {
            eprintln!("closing a shared buffer: {error}");
        }
    }
}

/// Closes the WebView2 buffers posted for `session_id`, leaving probes and any other session untouched.
pub fn release_session_buffers(window: &WebviewWindow, session_id: u32) {
    let _ = window.with_webview(move |_| {
        // SAFETY: `with_webview` runs this closure on the WebView's UI thread.
        unsafe { close_live_buffers(Some(BufferScope::Session(session_id))) };
    });
}

/// Closes the small delivery-probe buffers on the UI thread.
pub fn release_probe_buffers(window: &WebviewWindow) {
    PROBES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let _ = window.with_webview(|_| {
        // SAFETY: `with_webview` runs this closure on the WebView's UI thread.
        unsafe { close_live_buffers(Some(BufferScope::Probe)) };
    });
}

/// Closes every tracked WebView2 buffer on the UI thread.
/// Failing to reach the UI thread means the app is exiting, when it no longer matters.
pub fn release_buffers(window: &WebviewWindow) {
    PROBES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let _ = window.with_webview(|_| {
        // SAFETY: `with_webview` runs this closure on the WebView's UI thread.
        unsafe { close_live_buffers(None) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draining_one_scope_leaves_other_sessions_and_probes_intact() {
        let mut tracked = vec![
            (BufferScope::Probe, 10),
            (BufferScope::Session(1), 11),
            (BufferScope::Session(1), 12),
            (BufferScope::Session(2), 21),
        ];

        let mut closed_session_1 =
            drain_matching_scope(&mut tracked, Some(BufferScope::Session(1)));
        closed_session_1.sort_unstable();
        assert_eq!(closed_session_1, vec![11, 12]);
        assert_eq!(
            tracked,
            vec![(BufferScope::Probe, 10), (BufferScope::Session(2), 21)]
        );

        let closed_probe = drain_matching_scope(&mut tracked, Some(BufferScope::Probe));
        assert_eq!(closed_probe, vec![10]);
        assert_eq!(tracked, vec![(BufferScope::Session(2), 21)]);

        let closed_all = drain_matching_scope(&mut tracked, None);
        assert_eq!(closed_all, vec![21]);
        assert!(tracked.is_empty());
    }
}
