//! Process mitigation policies, set at creation and impossible to turn off afterwards.

use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::ptr;

use windows_sys::Win32::System::SystemServices::{
    PROCESS_MITIGATION_ASLR_POLICY, PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY,
    PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY, PROCESS_MITIGATION_DYNAMIC_CODE_POLICY,
    PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY, PROCESS_MITIGATION_FONT_DISABLE_POLICY,
    PROCESS_MITIGATION_IMAGE_LOAD_POLICY, PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY,
    PROCESS_MITIGATION_SYSTEM_CALL_DISABLE_POLICY,
};
use windows_sys::Win32::System::Threading::{
    GetProcessMitigationPolicy, PROCESS_MITIGATION_POLICY, ProcessASLRPolicy,
    ProcessControlFlowGuardPolicy, ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy,
    ProcessFontDisablePolicy, ProcessImageLoadPolicy, ProcessSignaturePolicy,
    ProcessStrictHandleCheckPolicy, ProcessSystemCallDisablePolicy,
};

// `winbase.h` defines these as macros, which `windows-sys` does not carry. Each policy
// takes up a field of its own, where 1 means "always on".
const FORCE_RELOCATE_IMAGES: u64 = 1 << 8;
const HEAP_TERMINATE: u64 = 1 << 12;
const BOTTOM_UP_ASLR: u64 = 1 << 16;
const HIGH_ENTROPY_ASLR: u64 = 1 << 20;
const STRICT_HANDLE_CHECKS: u64 = 1 << 24;
const WIN32K_DISABLE: u64 = 1 << 28;
const EXTENSION_POINT_DISABLE: u64 = 1 << 32;
const PROHIBIT_DYNAMIC_CODE: u64 = 1 << 36;
const CONTROL_FLOW_GUARD: u64 = 1 << 40;
const MICROSOFT_SIGNED_ONLY: u64 = 1 << 44;
const FONT_DISABLE: u64 = 1 << 48;
const IMAGE_LOAD_NO_REMOTE: u64 = 1 << 52;
const IMAGE_LOAD_NO_LOW_LABEL: u64 = 1 << 56;
const IMAGE_LOAD_PREFER_SYSTEM32: u64 = 1 << 60;

const FSCTL_SYSTEM_CALL_DISABLE: u64 = 1 << 56;

/// The value of `PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY` that forbids child processes.
pub(crate) const CHILD_PROCESS_RESTRICTED: u32 = 1;

/// The two words of `PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY` for a locked-down process.
///
/// Worth noting what is absent, because `pdfium.dll` rules it out. Blocking binaries not signed
/// by Microsoft would reject it. Disabling win32k would stop it from loading, since it imports
/// `user32.dll`, whose initialization fails without win32k, and asking for it after loading is
/// refused with access denied. Both were tried on Windows 11. A PDFium built with those imports
/// delay-loaded would allow it.
pub(crate) const LOCKDOWN_POLICY: [u64; 2] = [
    FORCE_RELOCATE_IMAGES
        | HEAP_TERMINATE
        | BOTTOM_UP_ASLR
        | HIGH_ENTROPY_ASLR
        | STRICT_HANDLE_CHECKS
        | EXTENSION_POINT_DISABLE
        | PROHIBIT_DYNAMIC_CODE
        | CONTROL_FLOW_GUARD
        | FONT_DISABLE
        | IMAGE_LOAD_NO_REMOTE
        | IMAGE_LOAD_NO_LOW_LABEL
        | IMAGE_LOAD_PREFER_SYSTEM32,
    FSCTL_SYSTEM_CALL_DISABLE,
];

/// The same, plus what `pdfium.dll` rules out above, for a process that only parses bytes and so
/// has no use for windows or for a DLL Microsoft did not sign.
pub(crate) const PARSER_POLICY: [u64; 2] = [
    LOCKDOWN_POLICY[0] | WIN32K_DISABLE | MICROSOFT_SIGNED_ONLY,
    LOCKDOWN_POLICY[1],
];

/// Which mitigations a running process really has, read back from the kernel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MitigationReport {
    /// It cannot create executable memory or make memory executable.
    pub dynamic_code_prohibited: bool,
    /// `ForceRelocateImages`, bottom-up and high-entropy ASLR.
    pub aslr_forced: bool,
    /// Using an invalid handle kills the process instead of returning an error.
    pub strict_handle_checks: bool,
    /// AppInit DLLs and similar legacy injection points are off.
    pub extension_points_disabled: bool,
    /// GDI cannot load fonts from outside the system's font folder.
    pub non_system_fonts_disabled: bool,
    /// It cannot load images from remote shares or marked as low integrity.
    pub image_load_restricted: bool,
    /// Indirect calls are checked against the targets each module declares.
    pub control_flow_guard: bool,
    /// It cannot call into the kernel's window and graphics subsystem.
    pub win32k_disabled: bool,
    /// It cannot load a DLL that Microsoft did not sign.
    pub microsoft_signed_only: bool,
}

impl MitigationReport {
    /// Reads the policies of `process`, which needs query access.
    pub fn read(process: BorrowedHandle<'_>) -> io::Result<Self> {
        let dynamic: PROCESS_MITIGATION_DYNAMIC_CODE_POLICY =
            policy(process, ProcessDynamicCodePolicy)?;
        let aslr: PROCESS_MITIGATION_ASLR_POLICY = policy(process, ProcessASLRPolicy)?;
        let handles: PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY =
            policy(process, ProcessStrictHandleCheckPolicy)?;
        let extensions: PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY =
            policy(process, ProcessExtensionPointDisablePolicy)?;
        let fonts: PROCESS_MITIGATION_FONT_DISABLE_POLICY =
            policy(process, ProcessFontDisablePolicy)?;
        let images: PROCESS_MITIGATION_IMAGE_LOAD_POLICY = policy(process, ProcessImageLoadPolicy)?;
        let guard: PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY =
            policy(process, ProcessControlFlowGuardPolicy)?;
        let win32k: PROCESS_MITIGATION_SYSTEM_CALL_DISABLE_POLICY =
            policy(process, ProcessSystemCallDisablePolicy)?;
        let signature: PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY =
            policy(process, ProcessSignaturePolicy)?;

        // SAFETY: each of these is a union of plain integers, and `Flags` covers all of it.
        // The bit positions are the ones `winnt.h` documents for each policy's bitfield.
        let flags = unsafe {
            [
                dynamic.Anonymous.Flags,
                aslr.Anonymous.Flags,
                handles.Anonymous.Flags,
                extensions.Anonymous.Flags,
                fonts.Anonymous.Flags,
                images.Anonymous.Flags,
                guard.Anonymous.Flags,
                win32k.Anonymous.Flags,
                signature.Anonymous.Flags,
            ]
        };
        Ok(Self {
            dynamic_code_prohibited: flags[0] & 1 != 0,
            // EnableBottomUpRandomization, EnableForceRelocateImages, EnableHighEntropy.
            aslr_forced: flags[1] & 0b111 == 0b111,
            // RaiseExceptionOnInvalidHandleReference, HandleExceptionsPermanentlyEnabled.
            strict_handle_checks: flags[2] & 0b11 == 0b11,
            extension_points_disabled: flags[3] & 1 != 0,
            non_system_fonts_disabled: flags[4] & 1 != 0,
            // NoRemoteImages, NoLowMandatoryLabelImages.
            image_load_restricted: flags[5] & 0b11 == 0b11,
            control_flow_guard: flags[6] & 1 != 0,
            win32k_disabled: flags[7] & 1 != 0,
            microsoft_signed_only: flags[8] & 1 != 0,
        })
    }
}

fn policy<T>(process: BorrowedHandle<'_>, class: PROCESS_MITIGATION_POLICY) -> io::Result<T> {
    // SAFETY: every `T` used here is a union of plain integers, for which zeroes are valid.
    let mut value: T = unsafe { mem::zeroed() };
    // SAFETY: the borrow keeps the handle open, and each caller pairs the class with its `T`.
    let ok = unsafe {
        GetProcessMitigationPolicy(
            process.as_raw_handle(),
            class,
            ptr::from_mut(&mut value).cast(),
            mem::size_of::<T>(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}
