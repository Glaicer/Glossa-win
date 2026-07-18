//! Shared Win32 dialog chrome: ComCtl v6 visual styles, fonts, palette, sizing.

#![cfg(windows)]

use std::{
    mem::size_of,
    os::windows::ffi::OsStrExt,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{COLORREF, HANDLE, HWND, RECT, INVALID_HANDLE_VALUE},
    Graphics::{
        Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
        Gdi::{
            CreateFontW, GetStockObject, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
            DEFAULT_GUI_FONT, FW_NORMAL, FW_SEMIBOLD, HFONT, OUT_DEFAULT_PRECIS,
        },
    },
    System::ApplicationInstallationAndServicing::{
        ActivateActCtx, CreateActCtxW, DeactivateActCtx, ReleaseActCtx, ACTCTXW,
    },
    UI::{
        Controls::{
            InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX,
        },
        WindowsAndMessaging::{
            AdjustWindowRectEx, GetSystemMetrics, GetWindowRect, SetWindowPos, SM_CXSCREEN,
            SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_EX_CONTROLPARENT,
            WS_OVERLAPPED, WS_SYSMENU,
        },
    },
};

pub const WINDOW_STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
pub const WINDOW_EX_STYLE: u32 = WS_EX_CONTROLPARENT;

// Light palette (Windows 11 settings-inspired).
pub const LIGHT_BG: COLORREF = rgb(243, 243, 243);
pub const LIGHT_TEXT: COLORREF = rgb(28, 28, 28);
pub const LIGHT_EDIT_BG: COLORREF = rgb(255, 255, 255);

// Dark palette (used by theme-aware dialogs such as Settings).
#[allow(dead_code)]
pub const DARK_BG: COLORREF = rgb(32, 32, 32);
#[allow(dead_code)]
pub const DARK_TEXT: COLORREF = rgb(250, 250, 250);
#[allow(dead_code)]
pub const DARK_EDIT_BG: COLORREF = rgb(45, 45, 45);

pub const fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    r | (g << 8) | (b << 16)
}

pub fn to_wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Isolation context that loads Common Controls v6 for themed Win32 chrome.
/// Dropping deactivates and releases the context.
pub struct VisualStylesGuard {
    cookie: usize,
    actctx: HANDLE,
}

impl Drop for VisualStylesGuard {
    fn drop(&mut self) {
        // SAFETY: cookie/actctx from a matching ActivateActCtx / CreateActCtx pair.
        unsafe {
            if self.cookie != 0 {
                let _ = DeactivateActCtx(0, self.cookie);
                self.cookie = 0;
            }
            if !self.actctx.is_null() && self.actctx != INVALID_HANDLE_VALUE {
                ReleaseActCtx(self.actctx);
                self.actctx = ptr::null_mut();
            }
        }
    }
}

/// Activates ComCtl32 v6 via a temporary assembly manifest.
///
/// Works even when the process was not linked with a Common-Controls
/// dependency (the main reason classic dialogs look like Windows 95).
pub unsafe fn activate_visual_styles() -> Option<VisualStylesGuard> {
    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

    let dir = std::env::temp_dir().join("glossa-visual-styles");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("comctl.v6.manifest");
    if std::fs::write(&path, MANIFEST).is_err() {
        return None;
    }

    let path_wide = to_wide(path.to_string_lossy().as_ref());
    let act = ACTCTXW {
        cbSize: size_of::<ACTCTXW>() as u32,
        dwFlags: 0,
        lpSource: path_wide.as_ptr(),
        wProcessorArchitecture: 0,
        wLangId: 0,
        lpAssemblyDirectory: ptr::null(),
        lpResourceName: ptr::null(),
        lpApplicationName: ptr::null(),
        hModule: ptr::null_mut(),
    };

    let actctx = CreateActCtxW(&act);
    if actctx.is_null() || actctx == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut cookie: usize = 0;
    if ActivateActCtx(actctx, &mut cookie) == 0 {
        ReleaseActCtx(actctx);
        return None;
    }

    Some(VisualStylesGuard { cookie, actctx })
}

pub unsafe fn ensure_common_controls() {
    static INIT: AtomicBool = AtomicBool::new(false);
    if INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    let icc = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
    };
    let _ = InitCommonControlsEx(&icc);
}

pub unsafe fn apply_titlebar_theme(hwnd: HWND, dark: bool) {
    let use_dark: i32 = if dark { 1 } else { 0 };
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        &use_dark as *const i32 as *const _,
        size_of::<i32>() as u32,
    );
}

pub unsafe fn create_ui_font(weight: i32, height: i32) -> HFONT {
    let font = CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        0,
        to_wide("Segoe UI").as_ptr(),
    );
    if font.is_null() {
        GetStockObject(DEFAULT_GUI_FONT) as HFONT
    } else {
        font
    }
}

pub unsafe fn create_body_font() -> HFONT {
    create_ui_font(FW_NORMAL as i32, -15)
}

pub unsafe fn create_header_font() -> HFONT {
    create_ui_font(FW_SEMIBOLD as i32, -15)
}

/// Convert a desired client size into outer CreateWindow dimensions
/// (title bar + borders included).
pub unsafe fn outer_size_for_client(client_w: i32, client_h: i32) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    if AdjustWindowRectEx(&mut rect, WINDOW_STYLE, 0, WINDOW_EX_STYLE) != 0 {
        (rect.right - rect.left, rect.bottom - rect.top)
    } else {
        (client_w + 16, client_h + 40)
    }
}

/// Resize `hwnd` so its client area is exactly `client_w` × `client_h`.
pub unsafe fn fit_window_to_client(hwnd: HWND, client_w: i32, client_h: i32) {
    let (outer_w, outer_h) = outer_size_for_client(client_w, client_h);
    let _ = SetWindowPos(
        hwnd,
        ptr::null_mut(),
        0,
        0,
        outer_w,
        outer_h,
        SWP_NOMOVE | SWP_NOZORDER,
    );
}

pub unsafe fn center_window(hwnd: HWND) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut rect) != 0 {
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let x = (screen_w - width) / 2;
        let y = (screen_h - height) / 2;
        let _ = SetWindowPos(hwnd, ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
    }
}
