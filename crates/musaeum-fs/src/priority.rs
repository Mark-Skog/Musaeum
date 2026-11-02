//! Background priority, for work the user must never feel.

use std::marker::PhantomData;

use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END,
};

/// While this exists, the thread that made it runs with low CPU, I/O and memory priority.
#[derive(Debug)]
pub struct BackgroundPriority {
    // The mode belongs to one thread, so this has to end on the thread that started it.
    _not_send: PhantomData<*const ()>,
}

/// Puts this thread in background mode until the returned value is dropped. If Windows refuses,
/// the work carries on at the normal priority, which is slower for others but still correct.
pub fn enter_background() -> BackgroundPriority {
    // SAFETY: the pseudo-handle is always valid, and this mode only applies to the calling thread.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
    BackgroundPriority {
        _not_send: PhantomData,
    }
}

impl Drop for BackgroundPriority {
    fn drop(&mut self) {
        // SAFETY: as above, and this value cannot have moved to another thread.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) };
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::Threading::{GetThreadPriority, THREAD_PRIORITY_NORMAL};

    use super::*;

    fn priority() -> i32 {
        // SAFETY: the pseudo-handle is always valid.
        unsafe { GetThreadPriority(GetCurrentThread()) }
    }

    #[test]
    fn the_mode_really_applies_and_really_ends() {
        std::thread::spawn(|| {
            assert_eq!(priority(), THREAD_PRIORITY_NORMAL);
            let background = enter_background();
            assert!(
                priority() < THREAD_PRIORITY_NORMAL,
                "reported: {}",
                priority()
            );
            drop(background);
            assert_eq!(priority(), THREAD_PRIORITY_NORMAL);
        })
        .join()
        .unwrap();
    }
}
