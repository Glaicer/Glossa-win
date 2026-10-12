//! Topmost progress pill shown while the updater installs a new release.
//!
//! A dedicated worker thread owns the window; the caller only sends progress
//! values (0-100) and the pill repaints. The window hides itself on drop.

use std::{
    cell::RefCell,
    ffi::c_void,
    sync::{mpsc, Once},
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
        FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
        UpdateWindow, DT_CENTER, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
        LoadCursorW, RegisterClassW, SetWindowPos, ShowWindow, SystemParametersInfoW,
        CS_DROPSHADOW, HWND_TOPMOST, IDC_ARROW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA,
        SWP_NOACTIVATE, SW_SHOWNOACTIVATE, WNDCLASSW, WM_DESTROY, WM_PAINT, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    },
};

use crate::win_style::{create_body_font, rgb};

const OVERLAY_CLASS: &str = "glossa-update-overlay";
const OVERLAY_WIDTH: i32 = 320;
const OVERLAY_HEIGHT: i32 = 72;
const CORNER_RADIUS: i32 = 18;
const SIDE_MARGIN: i32 = 24;
const BOTTOM_MARGIN: i32 = 28;

const BG: u32 = rgb(32, 32, 32);
const TEXT: u32 = rgb(250, 250, 250);
const BAR_TRACK: u32 = rgb(70, 70, 70);
const BAR_FILL: u32 = rgb(0, 103, 192);

enum OverlayCommand {
    Show(String),
    Progress(u8),
    Hide,
}

#[derive(Clone)]
struct OverlayState {
    label: String,
    percent: u8,
}

thread_local! {
    static OVERLAY_STATE: RefCell<Option<OverlayState>> = const { RefCell::new(None) };
}

/// Progress pill for the updater; best-effort (a dead worker disables the UI).
pub struct UpdateOverlay {
    command_tx: Option<mpsc::Sender<OverlayCommand>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl UpdateOverlay {
    /// Shows the pill at the bottom-center of the screen.
    #[must_use]
    pub fn show(label: &str) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        match thread::Builder::new()
            .name("glossa-update-overlay".into())
            .spawn(move || run_worker(command_rx))
        {
            Ok(thread_handle) => {
                let _ = command_tx.send(OverlayCommand::Show(label.to_owned()));
                Self {
                    command_tx: Some(command_tx),
                    thread_handle: Some(thread_handle),
                }
            }
            Err(_) => Self {
                command_tx: None,
                thread_handle: None,
            },
        }
    }

    /// Updates the progress bar (0-100).
    pub fn set_progress(&self, percent: u8) {
        if let Some(command_tx) = &self.command_tx {
            let _ = command_tx.send(OverlayCommand::Progress(percent));
        }
    }
}

impl Drop for UpdateOverlay {
    fn drop(&mut self) {
        if let Some(command_tx) = self.command_tx.take() {
            let _ = command_tx.send(OverlayCommand::Hide);
        }
        if let Some(thread_handle) = self.thread_handle.take() {
            let _ = thread_handle.join();
        }
    }
}

fn run_worker(command_rx: mpsc::Receiver<OverlayCommand>) {
    // SAFETY: the worker thread owns the window it creates and destroys.
    let hwnd = unsafe { create_overlay_window() };

    for command in command_rx {
        match command {
            OverlayCommand::Show(label) => {
                OVERLAY_STATE.with(|cell| {
                    *cell.borrow_mut() = Some(OverlayState {
                        label,
                        percent: 0,
                    });
                });
                if let Some(hwnd) = hwnd {
                    // SAFETY: hwnd is a live window owned by this thread.
                    unsafe {
                        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    }
                }
            }
            OverlayCommand::Progress(percent) => {
                OVERLAY_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.percent = percent.min(100);
                    }
                });
                if let Some(hwnd) = hwnd {
                    // SAFETY: hwnd is a live window owned by this thread.
                    unsafe {
                        repaint(hwnd);
                    }
                }
            }
            OverlayCommand::Hide => break,
        }
    }

    if let Some(hwnd) = hwnd {
        // SAFETY: hwnd is a window owned by this thread.
        unsafe {
            DestroyWindow(hwnd);
        }
    }
}

/// # Safety
/// Must run on the overlay worker thread only.
unsafe fn create_overlay_window() -> Option<HWND> {
    let hinstance = GetModuleHandleW(std::ptr::null());
    static REGISTER: Once = Once::new();
    let mut registered = true;
    REGISTER.call_once(|| {
        let class_name = to_wide(OVERLAY_CLASS);
        let wndclass = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(overlay_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&wndclass) == 0 {
            registered = false;
        }
    });
    if !registered {
        return None;
    }

    let class_name = to_wide(OVERLAY_CLASS);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        class_name.as_ptr(),
        std::ptr::null(),
        WS_POPUP,
        0,
        0,
        OVERLAY_WIDTH,
        OVERLAY_HEIGHT,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinstance,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        return None;
    }
    position_bottom_center(hwnd);
    Some(hwnd)
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_overlay(hwnd);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint_overlay(hwnd: HWND) {
    let Some(state) = OVERLAY_STATE.with(|cell| cell.borrow().clone()) else {
        return;
    };
    let mut ps = std::mem::zeroed::<PAINTSTRUCT>();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rect);

    let bg_brush = CreateSolidBrush(BG);
    FillRect(hdc, &rect, bg_brush);
    DeleteObject(bg_brush);

    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, TEXT);
    let font = create_body_font();
    let old_font = SelectObject(hdc, font);

    let mut label = to_wide(&state.label);
    let mut label_rect = RECT {
        left: SIDE_MARGIN,
        top: 16,
        right: rect.right - SIDE_MARGIN,
        bottom: 40,
    };
    DrawTextW(
        hdc,
        label.as_mut_ptr(),
        label.len().saturating_sub(1) as i32,
        &mut label_rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    SelectObject(hdc, old_font);
    DeleteObject(font);

    const BAR_HEIGHT: i32 = 10;
    const BAR_Y: i32 = 48;
    let track_w = rect.right - rect.left - SIDE_MARGIN * 2;
    let track_brush = CreateSolidBrush(BAR_TRACK);
    let track = RECT {
        left: rect.left + SIDE_MARGIN,
        top: BAR_Y,
        right: rect.left + SIDE_MARGIN + track_w,
        bottom: BAR_Y + BAR_HEIGHT,
    };
    FillRect(hdc, &track, track_brush);
    DeleteObject(track_brush);

    if state.percent > 0 {
        let fill_w = (track_w * i32::from(state.percent) / 100).max(4);
        let fill_brush = CreateSolidBrush(BAR_FILL);
        let fill = RECT {
            left: rect.left + SIDE_MARGIN,
            top: BAR_Y,
            right: rect.left + SIDE_MARGIN + fill_w,
            bottom: BAR_Y + BAR_HEIGHT,
        };
        FillRect(hdc, &fill, fill_brush);
        DeleteObject(fill_brush);
    }

    EndPaint(hwnd, &ps);
}

unsafe fn repaint(hwnd: HWND) {
    InvalidateRect(hwnd, std::ptr::null(), 1);
    UpdateWindow(hwnd);
}

unsafe fn position_bottom_center(hwnd: HWND) {
    let mut work_area = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: pvparam points at a RECT sized for SPI_GETWORKAREA.
    let have_work_area = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        &mut work_area as *mut RECT as *mut c_void,
        0,
    ) != 0
        && work_area.right > work_area.left;
    let (left, width, bottom) = if have_work_area {
        (
            work_area.left,
            work_area.right - work_area.left,
            work_area.bottom,
        )
    } else {
        (
            0,
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
        )
    };
    let x = left + (width - OVERLAY_WIDTH) / 2;
    let y = bottom - OVERLAY_HEIGHT - BOTTOM_MARGIN;
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        OVERLAY_WIDTH,
        OVERLAY_HEIGHT,
        SWP_NOACTIVATE,
    );
    // Rounded corners via a window region; the system owns the region after this.
    let region = CreateRoundRectRgn(
        0,
        0,
        OVERLAY_WIDTH + 1,
        OVERLAY_HEIGHT + 1,
        CORNER_RADIUS,
        CORNER_RADIUS,
    );
    if !region.is_null() {
        SetWindowRgn(hwnd, region, 1);
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
