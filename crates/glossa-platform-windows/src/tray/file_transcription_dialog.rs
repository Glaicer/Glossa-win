//! Native Win32 "Transcribe File" window (tray menu → Transcribe File).
//!
//! Runs non-modally on a dedicated UI thread so the tray and the daemon keep
//! working while a transcription is in flight; closing the window aborts it.

use std::sync::Arc;

use glossa_app::{ports::ClipboardWriter, services::file_transcription::FileTranscriptionService};
use glossa_core::UiTheme;
use tokio::runtime::Handle;

/// Tray-side handle for the Transcribe File window. `show` is fire-and-forget:
/// the window opens on its own thread and the tray loop continues.
#[derive(Clone)]
pub struct FileTranscriptionUi {
    service: Arc<FileTranscriptionService>,
    clipboard: Arc<dyn ClipboardWriter>,
    runtime: Handle,
    theme: UiTheme,
}

impl FileTranscriptionUi {
    pub fn new(
        service: Arc<FileTranscriptionService>,
        clipboard: Arc<dyn ClipboardWriter>,
        runtime: Handle,
        theme: UiTheme,
    ) -> Self {
        Self {
            service,
            clipboard,
            runtime,
            theme,
        }
    }

    /// Theme follows the tray (refreshed after a Settings change).
    pub(super) fn set_theme(&mut self, theme: UiTheme) {
        self.theme = theme;
    }

    /// Opens the window and returns immediately.
    pub fn show(&self) {
        #[cfg(windows)]
        ui::show_window(self.clone());
        #[cfg(not(windows))]
        let _ = self;
    }
}

#[cfg(windows)]
mod ui {
    use std::{
        mem::size_of,
        path::PathBuf,
        ptr,
        sync::{
            atomic::{AtomicI32, Ordering},
            Arc, Mutex,
        },
        thread,
    };

    use glossa_app::{services::file_transcription::validate_transcription_file, AppError};
    use glossa_core::UiTheme;
    use tokio::task::AbortHandle;
    use tracing::warn;
    use windows_sys::Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Dwm::DwmSetWindowAttribute,
            Gdi::{
                CreateSolidBrush, DeleteObject, FillRect, FrameRect, SetBkColor, SetBkMode,
                SetTextColor, HBRUSH, HFONT, TRANSPARENT,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                TTF_IDISHWND, TTF_SUBCLASS, TTTOOLINFOW, TOOLTIPS_CLASSW, TTM_ADDTOOLW,
                TTM_UPDATETIPTEXTW, TTS_NOPREFIX,
            },
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
                GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
                IsDialogMessageW, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW,
                SendMessageW, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage,
                UIS_SET, UISF_HIDEFOCUS, WM_UPDATEUISTATE, BS_PUSHBUTTON, CS_HREDRAW,
                CS_VREDRAW, CW_USEDEFAULT, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY,
                GWLP_USERDATA, IDC_ARROW, MSG, SW_HIDE, SW_SHOW, WM_APP, WM_CLOSE, WM_COMMAND,
                WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT,
                WNDCLASSEXW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VSCROLL,
                WS_VISIBLE,
            },
        },
    };

    use crate::win_style::{
        activate_visual_styles, apply_titlebar_theme, center_window, create_body_font,
        create_header_font, ensure_common_controls, outer_size_for_client, rgb, to_wide, DARK_BG,
        DARK_EDIT_BG, DARK_TEXT, LIGHT_BG, LIGHT_EDIT_BG, LIGHT_TEXT, WINDOW_EX_STYLE,
        WINDOW_STYLE,
    };

    const CLASS_NAME: &str = "GlossaTranscribeDialog";
    /// Desired *client* area size (outer frame is added via AdjustWindowRectEx).
    const WIN_WIDTH: i32 = 760;
    const WIN_HEIGHT: i32 = 560;

    const ID_CHOOSE: i32 = 3001;
    const ID_TRANSCRIBE: i32 = 3002;
    const ID_CLEAR: i32 = 3003;
    const ID_COPY: i32 = 3004;
    const ID_CLOSE: i32 = 3005;
    const ID_RESULT: i32 = 3006;
    /// IsDialogMessageW maps Esc to a WM_COMMAND notification with this id.
    const ID_CANCEL: i32 = 2;

    /// App-message kinds posted back from tokio tasks on completion.
    const WM_APP_TRANSCRIBE_DONE: u32 = WM_APP + 1;
    const WM_APP_COPIED: u32 = WM_APP + 2;

    const TRANSCRIBE_LABEL: &str = "Transcribe";
    const RETRY_LABEL: &str = "Retry";

    // STATIC style extras not exposed by windows-sys.
    const SS_PATHELLIPSIS: u32 = 0x0000_4000;
    const SS_EDITCONTROL: u32 = 0x0000_2000;

    // Multiline edit formatting-rect message for text padding (not exposed by
    // windows-sys): EM_SETRECT sets where text renders inside the edit control.
    const EM_SETRECT: u32 = 0x00B3;

    // Win11 rounded window corners.
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;

    // Result field border colors (subtle modern 1 px frame).
    const LIGHT_BORDER: COLORREF = rgb(203, 203, 203);
    const DARK_BORDER: COLORREF = rgb(86, 86, 86);

    unsafe fn hide_focus_indicator(hwnd: HWND) {
        SendMessageW(
            hwnd,
            WM_UPDATEUISTATE,
            (UIS_SET | (UISF_HIDEFOCUS << 16)) as WPARAM,
            0,
        );
    }
    pub(super) fn show_window(window: super::FileTranscriptionUi) {
        let result = thread::Builder::new()
            .name("glossa-transcribe".into())
            .spawn(move || {
                // SAFETY: window creation and message pump on the dedicated
                // thread that owns the window.
                unsafe { run_window(window) }
            });
        if let Err(error) = result {
            warn!(error = %error, "failed to spawn Transcribe File window thread");
        }
    }

    unsafe fn run_window(window: super::FileTranscriptionUi) -> Result<(), AppError> {
        let _visual_styles = activate_visual_styles();
        ensure_common_controls();

        static NEXT_CLASS_ID: AtomicI32 = AtomicI32::new(1);
        let class_name = format!(
            "{CLASS_NAME}{}",
            NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed)
        );
        let class_wide = to_wide(&class_name);
        let hinstance = GetModuleHandleW(ptr::null());
        let dark = matches!(window.theme, UiTheme::Dark);

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            // Background is painted in WM_ERASEBKGND with a themed brush.
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_wide.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(AppError::message(
                "failed to register Glossa transcribe window class",
            ));
        }

        let title = to_wide("Transcribe File");
        let (outer_w, outer_h) = outer_size_for_client(WIN_WIDTH, WIN_HEIGHT);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE,
            class_wide.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            &window as *const super::FileTranscriptionUi as *const _,
        );
        if hwnd.is_null() {
            return Err(AppError::message(
                "failed to create Glossa transcribe window",
            ));
        }

        apply_titlebar_theme(hwnd, dark);
        // SAFETY: attribute setter for this window's frame corner rounding.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &DWMWCP_ROUND as *const i32 as *const _,
                size_of::<i32>() as u32,
            );
        }
        center_window(hwnd);
        ShowWindow(hwnd, SW_SHOW);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            // Dialog-style keyboard on a plain window: Tab cycles focus and
            // Esc arrives as an IDCANCEL WM_COMMAND.
            if msg.hwnd.is_null() || IsDialogMessageW(hwnd, &msg) == 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    struct TranscribeState {
        ui: super::FileTranscriptionUi,
        hwnd: HWND,
        font: HFONT,
        header_font: HFONT,
        bg_brush: HBRUSH,
        edit_brush: HBRUSH,
        dark: bool,
        file: Option<PathBuf>,
        task: Option<AbortHandle>,
        choose: HWND,
        filename: HWND,
        tooltip: HWND,
        transcribe: HWND,
        clear: HWND,
        result: HWND,
        /// Frame rect (client coords) the parent draws around `result`.
        result_rect: RECT,
        status: HWND,
        copy: HWND,
        /// Written by the tokio task before it posts the matching WM_APP event.
        transcribe_result: Arc<Mutex<Option<Result<String, String>>>>,
        copy_result: Arc<Mutex<Option<Result<(), String>>>>,
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let create = &*(lparam
                    as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
                let window = &*(create.lpCreateParams as *const super::FileTranscriptionUi);
                on_create(hwnd, window);
                0
            }
            WM_ERASEBKGND => {
                let themed = with_state(hwnd, |state| {
                    (state.bg_brush, state.result_rect, state.dark)
                });
                if let Some((brush, border_rect, dark)) =
                    themed.filter(|(brush, _, _)| !brush.is_null())
                {
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    if GetClientRect(hwnd, &mut rect) != 0 {
                        FillRect(wparam as _, &rect, brush);
                        // Seamless padding: fill the result field's interior with
                        // the edit background, then draw the 1 px frame over it.
                        let interior_brush =
                            CreateSolidBrush(if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG });
                        FillRect(wparam as _, &border_rect, interior_brush);
                        DeleteObject(interior_brush);
                        let color = if dark { DARK_BORDER } else { LIGHT_BORDER };
                        let border_brush = CreateSolidBrush(color);
                        FrameRect(wparam as _, &border_rect, border_brush);
                        DeleteObject(border_brush);
                        return 1;
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CTLCOLORSTATIC => {
                // The read-only result field reports as a static but paints
                // with edit colors; labels use the window background.
                let target = lparam as HWND;
                let themed = with_state(hwnd, |state| {
                    let is_result = target == state.result;
                    let text = if state.dark { DARK_TEXT } else { LIGHT_TEXT };
                    let bg = if is_result {
                        if state.dark {
                            DARK_EDIT_BG
                        } else {
                            LIGHT_EDIT_BG
                        }
                    } else if state.dark {
                        DARK_BG
                    } else {
                        LIGHT_BG
                    };
                    let brush = if is_result {
                        state.edit_brush
                    } else {
                        state.bg_brush
                    };
                    (brush, text, bg)
                });
                match themed {
                    Some((brush, text, bg)) => {
                        SetTextColor(wparam as _, text);
                        SetBkColor(wparam as _, bg);
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                        brush as LRESULT
                    }
                    None => 0,
                }
            }
            WM_COMMAND => {
                on_command(hwnd, loword(wparam) as i32, lparam);
                0
            }
            WM_APP_TRANSCRIBE_DONE => {
                on_transcribe_done(hwnd);
                0
            }
            WM_APP_COPIED => {
                on_copied(hwnd);
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                // Abort the in-flight job first so it cannot post to a dead window.
                with_state_mut(hwnd, |state| {
                    if let Some(task) = state.task.take() {
                        task.abort();
                    }
                });
                with_state_mut(hwnd, |state| {
                    if !state.font.is_null() {
                        DeleteObject(state.font);
                    }
                    if !state.header_font.is_null() {
                        DeleteObject(state.header_font);
                    }
                    if !state.bg_brush.is_null() {
                        DeleteObject(state.bg_brush);
                    }
                    if !state.edit_brush.is_null() {
                        DeleteObject(state.edit_brush);
                    }
                });
                take_state(hwnd);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn on_create(hwnd: HWND, window: &super::FileTranscriptionUi) {
        let dark = matches!(window.theme, UiTheme::Dark);
        let font = create_body_font();
        let header_font = create_header_font();
        let bg_brush = CreateSolidBrush(if dark { DARK_BG } else { LIGHT_BG });
        let edit_brush = CreateSolidBrush(if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG });

        let margin = 20i32;
        let btn_h = 32i32;
        let btn_w = 100i32;
        let clear_w = 90i32;
        let choose_w = 120i32;

        // --- Top row: [Choose file…] [filename] … [Clear] [Transcribe] ---
        let y_row = margin;
        let choose = create_button(
            hwnd,
            margin,
            y_row,
            choose_w,
            btn_h,
            ID_CHOOSE,
            "Choose file…",
            font,
        );
        let transcribe_x = WIN_WIDTH - margin - 16 - btn_w;
        let clear_x = transcribe_x - 10 - clear_w;
        let transcribe = create_button(
            hwnd,
            transcribe_x,
            y_row,
            btn_w,
            btn_h,
            ID_TRANSCRIBE,
            TRANSCRIBE_LABEL,
            font,
        );
        ShowWindow(transcribe, SW_HIDE);
        let clear = create_button(hwnd, clear_x, y_row, clear_w, btn_h, ID_CLEAR, "Clear", font);
        ShowWindow(clear, SW_HIDE);
        let filename_x = margin + choose_w + 12;
        let filename_w = clear_x - 12 - filename_x;
        let filename = create_label(
            hwnd,
            filename_x,
            y_row + 8,
            filename_w,
            20,
            "",
            3010,
            font,
            SS_PATHELLIPSIS,
        );

        // --- Result ---
        let y_title = y_row + btn_h + 14;
        create_label(
            hwnd,
            margin,
            y_title,
            200,
            22,
            "Transcription",
            3011,
            header_font,
            0,
        );
        let y_result = y_title + 26;
        let result_w = WIN_WIDTH - margin * 2 - 16;
        let btns_y = WIN_HEIGHT - margin - btn_h;
        let status_h = 40i32;
        let status_y = btns_y - 8 - status_h;
        let result_h = status_y - 6 - y_result;
        // The parent draws a 1 px frame on this rect and fills the interior
        // with the edit background; the edit sits 1 px inside, flush on all
        // sides, so the scrollbar touches the frame. Text padding comes from
        // the EM_SETRECT formatting rectangle, not the window position.
        let result_rect = RECT {
            left: margin,
            top: y_result,
            right: margin + result_w,
            bottom: y_result + result_h,
        };
        let result = CreateWindowExW(
            0,
            to_wide("EDIT").as_ptr(),
            to_wide("").as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            margin + 1,
            y_result + 1,
            result_w - 2,
            result_h - 2,
            hwnd,
            ID_RESULT as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(result, WM_SETFONT, font as WPARAM, 1);
        // Padding around the transcription text: inset the formatting
        // rectangle inside the edit (the window stays flush for the scrollbar).
        let format_rect = RECT {
            left: 8,
            top: 8,
            right: result_w - 2 - 8,
            bottom: result_h - 2 - 8,
        };
        SendMessageW(
            result,
            EM_SETRECT,
            0,
            &format_rect as *const RECT as LPARAM,
        );
        hide_focus_indicator(result);

        // --- Status ---
        let status = create_label(
            hwnd,
            margin,
            status_y,
            result_w,
            status_h,
            "",
            3012,
            font,
            SS_EDITCONTROL,
        );

        // --- Action bar: [Copy] [Close] ---
        let copy_x = transcribe_x - 10 - btn_w;
        let copy = create_button(hwnd, copy_x, btns_y, btn_w, btn_h, ID_COPY, "Copy", font);
        EnableWindow(copy, 0);
        let close = create_button(
            hwnd,
            transcribe_x,
            btns_y,
            btn_w,
            btn_h,
            ID_CLOSE,
            "Close",
            font,
        );

        let tooltip = create_filename_tooltip(hwnd, filename);

        // No blue focus outlines: hide the focus indicator on every control
        // (the state propagates to children and survives focus changes).
        for control in [choose, transcribe, clear, result, copy, close] {
            hide_focus_indicator(control);
        }
        hide_focus_indicator(hwnd);

        let state = Box::new(TranscribeState {
            ui: window.clone(),
            hwnd,
            font,
            header_font,
            bg_brush,
            edit_brush,
            dark,
            file: None,
            task: None,
            choose,
            filename,
            tooltip,
            transcribe,
            clear,
            result,
            result_rect,
            status,
            copy,
            transcribe_result: Arc::new(Mutex::new(None)),
            copy_result: Arc::new(Mutex::new(None)),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        SetFocus(choose);
    }

    unsafe fn on_command(hwnd: HWND, id: i32, lparam: LPARAM) {
        // Push buttons only send BN_CLICKED; notifications from the read-only
        // result field (EN_*) carry its own id and fall through unused.
        let from_control = lparam != 0;
        match id {
            ID_CHOOSE if from_control => handle_choose(hwnd),
            ID_TRANSCRIBE if from_control => {
                with_state_mut(hwnd, |state| handle_transcribe(state));
            }
            ID_CLEAR if from_control => {
                with_state_mut(hwnd, |state| clear_result(state));
            }
            ID_COPY if from_control => {
                with_state_mut(hwnd, |state| handle_copy(state));
            }
            ID_CLOSE if from_control => {
                DestroyWindow(hwnd);
            }
            ID_CANCEL => {
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }

    unsafe fn handle_choose(hwnd: HWND) {
        let Some(path) = pick_audio_file(hwnd) else {
            return;
        };
        with_state_mut(hwnd, |state| {
            clear_result(state);
            match validate_transcription_file(&path) {
                Ok(_) => {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    SetWindowTextW(state.filename, to_wide(&name).as_ptr());
                    update_tooltip_text(
                        state.tooltip,
                        state.hwnd,
                        state.filename,
                        &path.to_string_lossy(),
                    );
                    state.file = Some(path);
                    ShowWindow(state.transcribe, SW_SHOW);
                    set_status(state, "");
                }
                Err(error) => {
                    state.file = None;
                    SetWindowTextW(state.filename, to_wide("").as_ptr());
                    update_tooltip_text(state.tooltip, state.hwnd, state.filename, "");
                    ShowWindow(state.transcribe, SW_HIDE);
                    set_status(state, &error.to_string());
                }
            }
        });
    }

    unsafe fn handle_transcribe(state: &mut TranscribeState) {
        let Some(path) = state.file.clone() else {
            return;
        };
        EnableWindow(state.choose, 0);
        EnableWindow(state.transcribe, 0);
        EnableWindow(state.copy, 0);
        ShowWindow(state.clear, SW_HIDE);
        SetWindowTextW(state.result, to_wide("").as_ptr());
        set_status(state, "Transcribing…");

        let ui = state.ui.clone();
        let slot = state.transcribe_result.clone();
        // Window handles are not Send; carry the raw value across threads.
        let hwnd_value = state.hwnd as isize;
        let job = ui.runtime.spawn(async move {
            let result = ui
                .service
                .transcribe(&path)
                .await
                .map_err(|error| error.to_string());
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(result);
            }
            // SAFETY: PostMessage is thread-safe; a stale handle (window
            // already closed) simply fails to deliver.
            let hwnd = hwnd_value as HWND;
            unsafe { PostMessageW(hwnd, WM_APP_TRANSCRIBE_DONE, 0, 0) };
        });
        state.task = Some(job.abort_handle());
    }

    unsafe fn on_transcribe_done(hwnd: HWND) {
        with_state_mut(hwnd, |state| {
            let result = state
                .transcribe_result
                .lock()
                .ok()
                .and_then(|mut guard| guard.take());
            state.task = None;
            EnableWindow(state.choose, 1);
            EnableWindow(state.transcribe, 1);
            match result {
                Some(Ok(text)) => {
                    SetWindowTextW(state.result, to_wide(&text).as_ptr());
                    EnableWindow(state.copy, 1);
                    SetWindowTextW(state.transcribe, to_wide(RETRY_LABEL).as_ptr());
                    ShowWindow(state.clear, SW_SHOW);
                    set_status(state, "");
                }
                Some(Err(error)) => set_status(state, &error),
                None => set_status(state, "Transcription failed: result was lost"),
            }
        });
    }

    unsafe fn handle_copy(state: &mut TranscribeState) {
        let text = get_text(state.result);
        if text.is_empty() {
            return;
        }
        EnableWindow(state.copy, 0);
        EnableWindow(state.choose, 0);
        EnableWindow(state.transcribe, 0);
        EnableWindow(state.clear, 0);

        let ui = state.ui.clone();
        let slot = state.copy_result.clone();
        // Window handles are not Send; carry the raw value across threads.
        let hwnd_value = state.hwnd as isize;
        let job = ui.runtime.spawn(async move {
            let result = ui
                .clipboard
                .set_text(&text)
                .await
                .map_err(|error| error.to_string());
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(result);
            }
            // SAFETY: PostMessage is thread-safe; a stale handle (window
            // already closed) simply fails to deliver.
            let hwnd = hwnd_value as HWND;
            unsafe { PostMessageW(hwnd, WM_APP_COPIED, 0, 0) };
        });
        state.task = Some(job.abort_handle());
    }

    unsafe fn on_copied(hwnd: HWND) {
        with_state_mut(hwnd, |state| {
            let result = state
                .copy_result
                .lock()
                .ok()
                .and_then(|mut guard| guard.take());
            state.task = None;
            EnableWindow(state.copy, 1);
            EnableWindow(state.choose, 1);
            EnableWindow(state.transcribe, 1);
            EnableWindow(state.clear, 1);
            match result {
                Some(Ok(())) => set_status(state, "Copied"),
                Some(Err(error)) => set_status(state, &error),
                None => set_status(state, "Copy failed: result was lost"),
            }
        });
    }

    /// Empties the result while keeping the selected file (Linux parity).
    unsafe fn clear_result(state: &mut TranscribeState) {
        SetWindowTextW(state.result, to_wide("").as_ptr());
        EnableWindow(state.copy, 0);
        SetWindowTextW(state.transcribe, to_wide(TRANSCRIBE_LABEL).as_ptr());
        ShowWindow(state.clear, SW_HIDE);
        set_status(state, "");
    }

    unsafe fn set_status(state: &TranscribeState, text: &str) {
        SetWindowTextW(state.status, to_wide(text).as_ptr());
    }

    unsafe fn get_text(hwnd: HWND) -> String {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let read = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if read <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..read as usize])
    }

    /// Native open-file dialog filtered to the supported audio extensions.
    unsafe fn pick_audio_file(hwnd: HWND) -> Option<PathBuf> {
        use windows_sys::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };

        let filter = to_wide("Audio files\0*.wav;*.mp3;*.m4a;*.flac;*.ogg;*.aac\0All files\0*.*\0");
        let title = to_wide("Choose audio file");
        let mut buffer = [0u16; 32768];
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = hwnd;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buffer.as_mut_ptr();
        ofn.nMaxFile = buffer.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        if len == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
    }

    /// Adds a tooltip showing the full path when hovering the filename label.
    unsafe fn create_filename_tooltip(owner: HWND, target: HWND) -> HWND {
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            ptr::null(),
            WS_POPUP | TTS_NOPREFIX,
            0,
            0,
            0,
            0,
            owner,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        if tooltip.is_null() {
            return ptr::null_mut();
        }
        let mut tool: TTTOOLINFOW = std::mem::zeroed();
        tool.cbSize = size_of::<TTTOOLINFOW>() as u32;
        tool.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
        tool.hwnd = owner;
        tool.uId = target as usize;
        SendMessageW(tooltip, TTM_ADDTOOLW, 0, &tool as *const TTTOOLINFOW as LPARAM);
        tooltip
    }

    unsafe fn update_tooltip_text(tooltip: HWND, owner: HWND, target: HWND, text: &str) {
        if tooltip.is_null() {
            return;
        }
        let mut wide = to_wide(text);
        let mut tool: TTTOOLINFOW = std::mem::zeroed();
        tool.cbSize = size_of::<TTTOOLINFOW>() as u32;
        tool.hwnd = owner;
        tool.uId = target as usize;
        tool.lpszText = wide.as_mut_ptr();
        SendMessageW(
            tooltip,
            TTM_UPDATETIPTEXTW,
            0,
            &tool as *const TTTOOLINFOW as LPARAM,
        );
    }

    /// Removes and drops the boxed state; call only from WM_DESTROY.
    unsafe fn take_state(hwnd: HWND) {
        // SAFETY: the state box was stored in GWLP_USERDATA in WM_CREATE.
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TranscribeState;
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
            }
        }
    }

    unsafe fn with_state<R>(hwnd: HWND, f: impl FnOnce(&TranscribeState) -> R) -> Option<R> {
        // SAFETY: GWLP_USERDATA holds a valid Box<TranscribeState> after WM_CREATE.
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TranscribeState;
            if ptr.is_null() {
                None
            } else {
                Some(f(&*ptr))
            }
        }
    }

    unsafe fn with_state_mut<R>(
        hwnd: HWND,
        f: impl FnOnce(&mut TranscribeState) -> R,
    ) -> Option<R> {
        // SAFETY: GWLP_USERDATA holds a valid Box<TranscribeState> after WM_CREATE.
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TranscribeState;
            if ptr.is_null() {
                None
            } else {
                Some(f(&mut *ptr))
            }
        }
    }

    unsafe fn create_label(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        text: &str,
        id: i32,
        font: HFONT,
        extra_styles: u32,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            to_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | extra_styles,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn create_button(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        text: &str,
        font: HFONT,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    fn loword(value: usize) -> u16 {
        (value & 0xFFFF) as u16
    }
}
