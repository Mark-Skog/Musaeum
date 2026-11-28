//! Native Win32 custom title bar frame for the main desktop window.
//!
//! Instead of turning native decorations off (`decorations: false`), which strips Windows 10/11
//! Aero Snap, minimize/maximize animations, drop shadow and rounded corners, this module keeps
//! `WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`, extends the DWM
//! frame by `1px` at the top so Windows draws its native border/shadow/rounded corners, and
//! removes only the top non-client caption bar in `WM_NCCALCSIZE`.
//!
//! For client regions marked `-webkit-app-region: drag` in CSS, WebView2's child `HWND` returns
//! `HTTRANSPARENT`, letting `WM_NCHITTEST` reach our subclass so Windows natively handles top
//! edge/corner resizing (`HTTOP`, `HTTOPLEFT`, `HTTOPRIGHT`) and caption dragging, Aero Snap,
//! `Win + Arrows` and double-click maximize/restore (`HTCAPTION`).

use tauri::WebviewWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmDefWindowProc, DwmExtendFrameIntoClientArea};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HTCAPTION, HTCLIENT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    IsZoomed, NCCALCSIZE_PARAMS, SM_CXFRAME, SM_CXPADDEDBORDER, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_ACTIVATE,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_STYLECHANGED, WS_CAPTION, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
};

/// Unique identifier for our window subclass callback.
const SUBCLASS_ID: usize = 0x4D55_5354;
/// Logical height of the React `TitleBar` component (unified Acrobat-style tab bar), in CSS pixels.
const TITLEBAR_HEIGHT_CSS_PX: i32 = 40;
/// Logical height of the top resize hit strip when windowed, in CSS pixels.
const TOP_RESIZE_BAND_CSS_PX: i32 = 6;
/// Logical width of top-left and top-right diagonal resize corners, in CSS pixels.
const TOP_CORNER_WIDTH_CSS_PX: i32 = 14;
/// Standard Windows baseline DPI (100% scale).
const USER_DEFAULT_SCREEN_DPI: u32 = 96;

/// Scales logical CSS pixels to physical pixels for `dpi` with nearest-integer rounding.
#[inline]
fn scale_for_dpi(px: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 {
        USER_DEFAULT_SCREEN_DPI
    } else {
        dpi
    };
    (px * dpi as i32 + (USER_DEFAULT_SCREEN_DPI as i32 / 2)) / USER_DEFAULT_SCREEN_DPI as i32
}

/// Computes the top frame overhang that Windows pushes off-screen when a `WS_THICKFRAME` window
/// is maximized (`SM_CXFRAME + SM_CXPADDEDBORDER` at `dpi`).
#[inline]
fn maximized_top_inset(frame_metric: i32, padded_border: i32) -> i32 {
    (frame_metric + padded_border).max(0)
}

/// Queries the system's maximized frame overhang for `dpi`.
fn system_maximized_top_inset(dpi: u32) -> i32 {
    let dpi = if dpi == 0 {
        USER_DEFAULT_SCREEN_DPI
    } else {
        dpi
    };
    // SAFETY: `GetSystemMetricsForDpi` is a pure Win32 metric lookup safe for any positive DPI.
    let frame = unsafe { GetSystemMetricsForDpi(SM_CXFRAME, dpi) };
    // SAFETY: `GetSystemMetricsForDpi` is a pure Win32 metric lookup safe for any positive DPI.
    let pad = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    maximized_top_inset(frame, pad)
}

/// Queries `hwnd`'s current monitor DPI, falling back to `96` if unavailable.
fn window_dpi(hwnd: HWND) -> u32 {
    // SAFETY: `hwnd` is a valid window handle when called inside our subclass or setup.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        USER_DEFAULT_SCREEN_DPI
    } else {
        dpi
    }
}

/// Whether `style` retains the native resizable frame (`false` while in borderless `F11` fullscreen).
#[inline]
fn has_resizable_frame(style: u32) -> bool {
    (style & WS_THICKFRAME.0) != 0
}

/// Classifies a screen-space cursor point `(cursor_x, cursor_y)` inside `win_rect` when WebView2
/// yields hit-testing (`HTTRANSPARENT` on `-webkit-app-region: drag`).
fn classify_caption_hit(
    win_rect: RECT,
    cursor_x: i32,
    cursor_y: i32,
    dpi: u32,
    frame_inset: i32,
    maximized: bool,
    framed: bool,
) -> u32 {
    if !framed {
        return HTCLIENT;
    }
    if cursor_x < win_rect.left || cursor_x >= win_rect.right || cursor_y < win_rect.top {
        return HTCLIENT;
    }

    let titlebar_px = scale_for_dpi(TITLEBAR_HEIGHT_CSS_PX, dpi);

    if maximized {
        let visible_top = win_rect.top + frame_inset;
        if cursor_y >= visible_top && cursor_y < visible_top + titlebar_px {
            return HTCAPTION;
        }
        return HTCLIENT;
    }

    let resize_band = scale_for_dpi(TOP_RESIZE_BAND_CSS_PX, dpi).max(frame_inset.min(8));
    let corner_width = scale_for_dpi(TOP_CORNER_WIDTH_CSS_PX, dpi);

    if cursor_y < win_rect.top + resize_band {
        if cursor_x < win_rect.left + corner_width {
            return HTTOPLEFT;
        }
        if cursor_x >= win_rect.right - corner_width {
            return HTTOPRIGHT;
        }
        return HTTOP;
    }

    if cursor_y < win_rect.top + titlebar_px {
        return HTCAPTION;
    }

    HTCLIENT
}

/// Extends `1px` of DWM frame at the top when framed (preserving Windows 11 rounded corners and
/// drop shadow), or resets margins to `0` in borderless fullscreen.
fn sync_dwm_margins(hwnd: HWND, framed: bool) {
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyTopHeight: i32::from(framed),
        cyBottomHeight: 0,
    };
    // SAFETY: `hwnd` is a valid top-level window handle and `margins` is a valid stack struct.
    let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
}

/// Installs the custom Win32 title bar frame and subclass on `window`.
pub fn install(window: &WebviewWindow) -> Result<(), String> {
    let hwnd = window
        .hwnd()
        .map_err(|error| format!("reading main HWND: {error}"))?;

    // SAFETY: `hwnd` belongs to `window` on the main UI thread during Tauri setup.
    let current_style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let required =
        WS_THICKFRAME.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
    if (current_style & required) != required {
        // SAFETY: `hwnd` is a valid top-level window on this thread; adding standard frame bits is sound.
        let _ = unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, (current_style | required) as isize) };
    }

    sync_dwm_margins(hwnd, true);

    // SAFETY: `hwnd` is valid on this UI thread and `subclass_proc` has the exact `SUBCLASSPROC` ABI.
    let installed = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) };
    if !installed.as_bool() {
        return Err("SetWindowSubclass refused the main window".to_owned());
    }

    // Trigger an immediate `WM_NCCALCSIZE` pass so WebView2's child bounds cover the title bar area.
    // SAFETY: `hwnd` is valid; `SWP_FRAMECHANGED` with `NOMOVE | NOSIZE | NOZORDER | NOACTIVATE` only recalculates frame metrics.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    Ok(())
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 && lparam.0 != 0 => {
            // SAFETY: `hwnd` is valid in its own window procedure.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
            if !has_resizable_frame(style) {
                // Fullscreen (`F11`): let `DefSubclassProc` leave the full monitor rect untouched.
                // SAFETY: forwarding unaltered message parameters to the previous window procedure.
                return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            }

            let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
            // SAFETY: when `wparam != 0` in `WM_NCCALCSIZE`, Windows guarantees `lparam` points to a valid `NCCALCSIZE_PARAMS`.
            let original_top = unsafe { (*params).rgrc[0].top };

            // Let `DefWindowProcW` compute native left, right and bottom client edges (preserving
            // invisible resize borders when windowed and taskbar/work-area bounds when maximized).
            // SAFETY: forwarding unaltered message parameters to the previous window procedure.
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };

            // SAFETY: `hwnd` is valid.
            let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
            let next_top = if maximized {
                original_top + system_maximized_top_inset(window_dpi(hwnd))
            } else {
                original_top
            };
            // SAFETY: `params` remains valid for the duration of `WM_NCCALCSIZE`.
            unsafe {
                (*params).rgrc[0].top = next_top;
            }
            result
        }

        WM_NCHITTEST => {
            let mut dwm_hit = LRESULT(0);
            // SAFETY: `hwnd` is valid and `dwm_hit` is a valid stack out-pointer.
            if unsafe { DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut dwm_hit) }.as_bool() {
                return dwm_hit;
            }

            // SAFETY: forwarding unaltered message parameters to the previous window procedure.
            let base_hit = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if base_hit != LRESULT(HTCLIENT as isize) {
                return base_hit;
            }

            // SAFETY: `hwnd` is valid.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
            let mut win_rect = RECT::default();
            // SAFETY: `hwnd` is valid and `win_rect` is a valid stack `RECT`.
            if unsafe { GetWindowRect(hwnd, &mut win_rect) }.is_err() {
                return base_hit;
            }

            let cursor_x = (lparam.0 & 0xffff) as i16 as i32;
            let cursor_y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let dpi = window_dpi(hwnd);
            let frame_inset = system_maximized_top_inset(dpi);
            // SAFETY: `hwnd` is valid.
            let maximized = unsafe { IsZoomed(hwnd) }.as_bool();

            let hit = classify_caption_hit(
                win_rect,
                cursor_x,
                cursor_y,
                dpi,
                frame_inset,
                maximized,
                has_resizable_frame(style),
            );
            LRESULT(hit as isize)
        }

        WM_ACTIVATE | WM_STYLECHANGED => {
            // SAFETY: `hwnd` is valid.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
            sync_dwm_margins(hwnd, has_resizable_frame(style));
            // SAFETY: forwarding unaltered message parameters to the previous window procedure.
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }

        WM_NCDESTROY => {
            // SAFETY: `hwnd` is being destroyed on this UI thread; removing our subclass before teardown is required by Win32.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            // SAFETY: forwarding unaltered `WM_NCDESTROY` to the previous window procedure.
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }

        _ => {
            // SAFETY: forwarding unaltered message parameters to the previous window procedure.
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RECT: RECT = RECT {
        left: 100,
        top: 50,
        right: 1100,
        bottom: 750,
    };

    #[test]
    fn scales_metrics_and_maximized_inset_across_dpis() {
        assert_eq!(scale_for_dpi(TITLEBAR_HEIGHT_CSS_PX, 96), 40);
        assert_eq!(scale_for_dpi(TITLEBAR_HEIGHT_CSS_PX, 120), 50);
        assert_eq!(scale_for_dpi(TITLEBAR_HEIGHT_CSS_PX, 144), 60);
        assert_eq!(scale_for_dpi(TITLEBAR_HEIGHT_CSS_PX, 192), 80);
        assert_eq!(maximized_top_inset(4, 4), 8);
        assert_eq!(maximized_top_inset(6, 6), 12);
    }

    #[test]
    fn classifies_windowed_top_resize_edges_and_caption_band() {
        // 100% DPI (96): top 6px band (capped to min(8, frame_inset)=8 -> max(6,8)=8px)
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 105, 52, 96, 8, false, true),
            HTTOPLEFT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 1095, 52, 96, 8, false, true),
            HTTOPRIGHT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 53, 96, 8, false, true),
            HTTOP
        );
        // Below top resize band inside 40px titlebar (`50..90`) -> HTCAPTION
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 89, 96, 8, false, true),
            HTCAPTION
        );
        // At/below 40px titlebar (`y >= 90`) -> HTCLIENT
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 90, 96, 8, false, true),
            HTCLIENT
        );

        // 125% DPI (120): titlebar is 50px (`50..100`), resize band is `scale_for_dpi(6, 120) = 8px`, corner is `18px`
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 117, 57, 120, 10, false, true),
            HTTOPLEFT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 1082, 57, 120, 10, false, true),
            HTTOPRIGHT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 57, 120, 10, false, true),
            HTTOP
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 99, 120, 10, false, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 100, 120, 10, false, true),
            HTCLIENT
        );

        // 150% DPI (144): titlebar is 60px (`50..110`), resize band is `scale_for_dpi(6, 144) = 9px`, corner is `21px`
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 120, 58, 144, 12, false, true),
            HTTOPLEFT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 1079, 58, 144, 12, false, true),
            HTTOPRIGHT
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 58, 144, 12, false, true),
            HTTOP
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 109, 144, 12, false, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 110, 144, 12, false, true),
            HTCLIENT
        );
    }

    #[test]
    fn classifies_maximized_and_fullscreen_caption_regions() {
        let max_rect = RECT {
            left: -8,
            top: -8,
            right: 1928,
            bottom: 1048,
        };
        // 100% DPI (96): at screen y = 0..39 when maximized (`top + 8`), returns HTCAPTION rather than HTTOP
        assert_eq!(
            classify_caption_hit(max_rect, 400, 0, 96, 8, true, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(max_rect, 400, 39, 96, 8, true, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(max_rect, 400, 40, 96, 8, true, true),
            HTCLIENT
        );

        // 125% DPI (120, inset 10) and 150% DPI (144, inset 12) when maximized
        let max_125 = RECT {
            left: -10,
            top: -10,
            right: 1930,
            bottom: 1050,
        };
        assert_eq!(
            classify_caption_hit(max_125, 400, 49, 120, 10, true, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(max_125, 400, 50, 120, 10, true, true),
            HTCLIENT
        );

        let max_150 = RECT {
            left: -12,
            top: -12,
            right: 1932,
            bottom: 1052,
        };
        assert_eq!(
            classify_caption_hit(max_150, 400, 59, 144, 12, true, true),
            HTCAPTION
        );
        assert_eq!(
            classify_caption_hit(max_150, 400, 60, 144, 12, true, true),
            HTCLIENT
        );

        // In borderless fullscreen (`framed == false`), never hijacks client area
        assert_eq!(
            classify_caption_hit(SAMPLE_RECT, 500, 52, 96, 8, false, false),
            HTCLIENT
        );
    }
}
