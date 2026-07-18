//! Key-capture hotkey picker control and modal prompt dialog.
//!
//! Displays shortcuts as chips (`[CTRL] + [ALT] + [SPACE]`) and records a
//! canonical config string (`Ctrl+Alt+Space`) when the user presses a combo.

#![cfg(windows)]

use std::{
    cell::RefCell,
    mem::size_of,
    ptr,
    sync::Once,
};

use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
        InvalidateRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE,
        DT_VCENTER, HBRUSH, HFONT, PAINTSTRUCT, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{GetKeyState, SetFocus},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            GetMessageW, GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassExW,
            SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
            BN_CLICKED, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
            GWLP_USERDATA,
            IDC_ARROW, MSG, SW_HIDE, SW_SHOW, SWP_NOZORDER, WM_COMMAND, WM_CREATE,
            WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN, WM_KEYUP,
            WM_KILLFOCUS, WM_LBUTTONDOWN, WM_NCPAINT, WM_PAINT, WM_SETFOCUS, WM_SETFONT,
            WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN,
            WS_TABSTOP, WS_VISIBLE,
        },
    },
};

use crate::dialog::DialogError;
use crate::win_style::{
    activate_visual_styles, apply_titlebar_theme, center_window, create_body_font,
    create_header_font, ensure_common_controls, fit_window_to_client, outer_size_for_client,
    to_wide, LIGHT_BG, LIGHT_EDIT_BG, LIGHT_TEXT, WINDOW_EX_STYLE, WINDOW_STYLE, rgb,
};

use super::shortcut_parser::{
    format_chord_display, format_shortcut_config, format_shortcut_display, is_modifier_vk,
    parse_shortcut, vk_to_token,
};

const PICKER_CLASS: &str = "GlossaHotkeyPicker";
const PROMPT_CLASS: &str = "GlossaHotkeyPrompt";

const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_PICKER: i32 = 1001;
/// Child control id for the in-picker Reset button.
const ID_RESET_BTN: i32 = 1;

const RESET_BTN_W: i32 = 58;
const RESET_GAP: i32 = 6;

/// Hint shown while the picker is focused and empty / waiting for input.
const CAPTURE_HINT: &str = "Press a key combination…";

/// Custom message: LPARAM = *const u16 (NUL-terminated wide config string).
const HKM_SETSHORTCUT: u32 = 0x0400 + 60;
/// Custom message: returns length; caller uses GetWindowText-style buffer via helper.
const HKM_GETSHORTCUT: u32 = 0x0400 + 61;

// Focus ring accent (Windows 11 blue-ish).
const FOCUS_BORDER: COLORREF = rgb(0, 103, 192);
const NORMAL_BORDER: COLORREF = rgb(180, 180, 180);

struct PickerState {
    /// Canonical config value, e.g. `Ctrl+Alt+Space`.
    config: String,
    /// Value when the control was created (config/default). Reset restores this.
    baseline: String,
    /// Live chip string while keys are held (`[CTRL] + [ALT]`). `None` → show `config`.
    live_preview: Option<String>,
    /// True while an incomplete capture gesture is in progress (modifiers only).
    /// Cleared on successful commit so releasing keys keeps the committed display.
    gesture_active: bool,
    /// True after a primary key committed this gesture; ignore release until next gesture.
    committed: bool,
    reset_btn: HWND,
    font: HFONT,
    focused: bool,
    bg_brush: HBRUSH,
    focus_brush: HBRUSH,
    border_brush: HBRUSH,
}

/// Creates a child hotkey picker control.
///
/// # Safety
/// `parent` must be a valid window; the returned HWND is owned by the parent.
pub unsafe fn create_hotkey_picker(
    parent: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
    font: HFONT,
    initial_config: &str,
) -> HWND {
    ensure_picker_class();
    let hinstance = GetModuleHandleW(ptr::null());
    let class_wide = to_wide(PICKER_CLASS);
    let hwnd = CreateWindowExW(
        0,
        class_wide.as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | WS_CLIPCHILDREN,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        hinstance,
        ptr::null(),
    );
    if hwnd.is_null() {
        return ptr::null_mut();
    }

    let initial = normalize_initial(initial_config);
    let bg_brush = CreateSolidBrush(LIGHT_EDIT_BG);
    let focus_brush = CreateSolidBrush(FOCUS_BORDER);
    let border_brush = CreateSolidBrush(NORMAL_BORDER);

    // Reset sits inside the picker on the right; hidden until the value diverges.
    let reset_btn = CreateWindowExW(
        0,
        to_wide("BUTTON").as_ptr(),
        to_wide("Reset").as_ptr(),
        WS_CHILD | WS_TABSTOP | BS_PUSHBUTTON as u32, // not WS_VISIBLE until dirty
        0,
        0,
        RESET_BTN_W,
        h.saturating_sub(4).max(20),
        hwnd,
        ID_RESET_BTN as _,
        hinstance,
        ptr::null(),
    );
    if !reset_btn.is_null() {
        SendMessageW(reset_btn, WM_SETFONT, font as usize, 1);
    }

    let state = Box::new(PickerState {
        config: initial.clone(),
        baseline: initial,
        live_preview: None,
        gesture_active: false,
        committed: false,
        reset_btn,
        font,
        focused: false,
        bg_brush,
        focus_brush,
        border_brush,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
    layout_reset_button(hwnd);
    InvalidateRect(hwnd, ptr::null(), 1);
    hwnd
}

/// Returns the canonical config shortcut currently stored in the picker.
///
/// # Safety
/// `hwnd` must be a hotkey picker created by [`create_hotkey_picker`].
pub unsafe fn hotkey_picker_get(hwnd: HWND) -> String {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return String::new();
    }
    (*ptr).config.clone()
}

/// Sets the picker's config shortcut and refreshes the display.
///
/// # Safety
/// `hwnd` must be a hotkey picker created by [`create_hotkey_picker`].
pub unsafe fn hotkey_picker_set(hwnd: HWND, config: &str) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }
    (*ptr).config = normalize_initial(config);
    (*ptr).live_preview = None;
    (*ptr).gesture_active = false;
    (*ptr).committed = false;
    sync_reset_button(hwnd, ptr);
    InvalidateRect(hwnd, ptr::null(), 1);
}

/// Modal dialog: press a key combo, OK/Cancel. Returns `None` on cancel.
pub fn prompt_hotkey_dialog(
    title: &str,
    initial: &str,
) -> Result<Option<String>, DialogError> {
    unsafe { prompt_hotkey_dialog_impl(title, initial) }
}

fn normalize_initial(config: &str) -> String {
    let trimmed = config.trim();
    if trimmed.is_empty() {
        return "Ctrl+Alt+Space".into();
    }
    // Re-parse and re-format when possible so display is stable.
    if parse_shortcut(trimmed).is_ok() {
        // Keep user-facing Windows style if already that shape; otherwise leave as-is
        // (parse accepts portal style, which format_shortcut_display also handles).
        return trimmed.to_owned();
    }
    trimmed.to_owned()
}

fn ensure_picker_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = to_wide(PICKER_CLASS);
        // SAFETY: process-local window class registration.
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(picker_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: ptr::null_mut(),
            };
            let _ = RegisterClassExW(&class);
        }
    });
}

unsafe extern "system" fn picker_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SETFONT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
            if !ptr.is_null() {
                (*ptr).font = wparam as HFONT;
                if !(*ptr).reset_btn.is_null() {
                    SendMessageW((*ptr).reset_btn, WM_SETFONT, wparam, 0);
                }
                if lparam != 0 {
                    InvalidateRect(hwnd, ptr::null(), 1);
                }
            }
            0
        }
        HKM_SETSHORTCUT => {
            if lparam != 0 {
                let wide = lparam as *const u16;
                let len = (0..4096)
                    .position(|i| *wide.add(i) == 0)
                    .unwrap_or(0);
                let slice = std::slice::from_raw_parts(wide, len);
                let config = String::from_utf16_lossy(slice);
                hotkey_picker_set(hwnd, &config);
            }
            0
        }
        HKM_GETSHORTCUT => {
            // Not used externally; kept for symmetry. Prefer hotkey_picker_get.
            0
        }
        WM_GETDLGCODE => {
            // Want all keys so Tab/arrows/etc. can form shortcuts while focused.
            0x0004 | 0x0001 // DLGC_WANTALLKEYS | DLGC_WANTARROWS
        }
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
            0
        }
        WM_SETFOCUS => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
            if !ptr.is_null() {
                (*ptr).focused = true;
                // Keep committed display; only clear an incomplete live gesture.
                if !(*ptr).committed {
                    (*ptr).live_preview = None;
                    (*ptr).gesture_active = false;
                }
                InvalidateRect(hwnd, ptr::null(), 1);
            }
            0
        }
        WM_KILLFOCUS => {
            // Ignore focus loss to our own Reset button so the field stays "active".
            let next = wparam as HWND;
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
            if !ptr.is_null() && !(*ptr).reset_btn.is_null() && next == (*ptr).reset_btn {
                return 0;
            }
            if !ptr.is_null() {
                (*ptr).focused = false;
                if !(*ptr).committed {
                    (*ptr).live_preview = None;
                    (*ptr).gesture_active = false;
                } else {
                    // Keep showing committed config chips after focus leaves.
                    (*ptr).live_preview = None;
                    (*ptr).gesture_active = false;
                }
                InvalidateRect(hwnd, ptr::null(), 1);
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            if id == ID_RESET_BTN && (code == BN_CLICKED || code == 0) {
                handle_reset(hwnd);
            }
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            handle_key_down(hwnd, wparam as u32);
            0
        }
        WM_KEYUP | WM_SYSKEYUP => {
            handle_key_up(hwnd, wparam as u32);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_picker(hwnd);
            0
        }
        WM_NCPAINT => {
            // Let default draw border; focus ring is drawn in paint.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let state = Box::from_raw(ptr);
                if !state.bg_brush.is_null() {
                    DeleteObject(state.bg_brush);
                }
                if !state.focus_brush.is_null() {
                    DeleteObject(state.focus_brush);
                }
                if !state.border_brush.is_null() {
                    DeleteObject(state.border_brush);
                }
                // font is owned by the parent dialog — do not delete.
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn handle_reset(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }
    (*ptr).config = (*ptr).baseline.clone();
    (*ptr).live_preview = None;
    (*ptr).gesture_active = false;
    (*ptr).committed = false;
    sync_reset_button(hwnd, ptr);
    InvalidateRect(hwnd, ptr::null(), 1);
    SetFocus(hwnd);
}

unsafe fn handle_key_down(hwnd: HWND, vk: u32) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }

    // After a successful commit, ignore further key activity until all keys are
    // released (handled in key_up). This prevents auto-repeat / held modifiers
    // from wiping the committed chip display.
    if (*ptr).committed {
        return;
    }

    let (ctrl, alt, shift, win) = current_modifiers();

    // Modifier pressed: show live chips immediately (e.g. [CTRL], then [CTRL] + [ALT]).
    if is_modifier_vk(vk) {
        (*ptr).gesture_active = true;
        let preview = format_chord_display(ctrl, alt, shift, win, None);
        if preview.is_empty() {
            (*ptr).live_preview = None;
        } else {
            (*ptr).live_preview = Some(preview);
        }
        InvalidateRect(hwnd, ptr::null(), 1);
        return;
    }

    let Some(token) = vk_to_token(vk) else {
        // Unknown primary key — still show any held modifiers.
        if ctrl || alt || shift || win {
            (*ptr).gesture_active = true;
            (*ptr).live_preview = Some(format_chord_display(ctrl, alt, shift, win, None));
            InvalidateRect(hwnd, ptr::null(), 1);
        }
        return;
    };

    // Escape alone is reserved (cancels recording); require a modifier with Esc.
    if vk == 0x1B && !ctrl && !alt && !shift && !win {
        return;
    }

    // Bare letter/digit shortcuts steal typing; require a modifier unless F-key.
    let is_function_key = (0x70..=0x7B).contains(&vk);
    if !ctrl && !alt && !shift && !win && !is_function_key {
        return;
    }

    // Live: show full chord including the primary key as soon as it is pressed.
    let preview = format_chord_display(ctrl, alt, shift, win, Some(token));
    (*ptr).live_preview = Some(preview);

    let config = format_shortcut_config(ctrl, alt, shift, win, token);
    if parse_shortcut(&config).is_ok() {
        (*ptr).config = config;
        // Freeze display on the committed combo; releasing keys must not strip chips.
        (*ptr).committed = true;
        (*ptr).gesture_active = false;
        (*ptr).live_preview = None; // paint uses format_shortcut_display(config)
        sync_reset_button(hwnd, ptr);
    } else {
        (*ptr).gesture_active = true;
    }

    InvalidateRect(hwnd, ptr::null(), 1);
}

unsafe fn handle_key_up(hwnd: HWND, _vk: u32) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }

    // After commit: wait until everything is up, then allow a new gesture.
    // Display stays on the committed config the whole time.
    if (*ptr).committed {
        let (ctrl, alt, shift, win) = current_modifiers();
        if !ctrl && !alt && !shift && !win {
            (*ptr).committed = false;
        }
        return;
    }

    if !(*ptr).gesture_active {
        return;
    }

    let (ctrl, alt, shift, win) = current_modifiers();

    // Incomplete gesture (modifiers only): update preview as keys lift.
    if ctrl || alt || shift || win {
        (*ptr).live_preview = Some(format_chord_display(ctrl, alt, shift, win, None));
        InvalidateRect(hwnd, ptr::null(), 1);
        return;
    }

    // Aborted capture — restore committed/baseline display.
    (*ptr).gesture_active = false;
    (*ptr).live_preview = None;
    InvalidateRect(hwnd, ptr::null(), 1);
}

fn is_dirty(state: &PickerState) -> bool {
    !shortcuts_equivalent(&state.config, &state.baseline)
}

/// Compare shortcuts ignoring portal vs Windows style and trivial spacing.
fn shortcuts_equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Prefer parse equality when both are valid.
    match (parse_shortcut(a), parse_shortcut(b)) {
        (Ok(ha), Ok(hb)) => ha == hb,
        _ => {
            format_shortcut_display(a).eq_ignore_ascii_case(&format_shortcut_display(b))
        }
    }
}

unsafe fn sync_reset_button(hwnd: HWND, ptr: *mut PickerState) {
    if ptr.is_null() {
        return;
    }
    let dirty = is_dirty(&*ptr);
    layout_reset_button_state(hwnd, &*ptr, dirty);
}

unsafe fn layout_reset_button(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }
    let dirty = is_dirty(&*ptr);
    layout_reset_button_state(hwnd, &*ptr, dirty);
}

unsafe fn layout_reset_button_state(hwnd: HWND, state: &PickerState, dirty: bool) {
    if state.reset_btn.is_null() {
        return;
    }
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetClientRect(hwnd, &mut rect) == 0 {
        return;
    }
    let height = rect.bottom - rect.top;
    let btn_h = (height - 4).max(20);
    let btn_y = (height - btn_h) / 2;
    let btn_x = rect.right - RESET_BTN_W - 3;
    let _ = SetWindowPos(
        state.reset_btn,
        ptr::null_mut(),
        btn_x,
        btn_y,
        RESET_BTN_W,
        btn_h,
        SWP_NOZORDER,
    );
    ShowWindow(state.reset_btn, if dirty { SW_SHOW } else { SW_HIDE });
}

fn current_modifiers() -> (bool, bool, bool, bool) {
    let ctrl = key_down(0x11); // VK_CONTROL
    let alt = key_down(0x12); // VK_MENU
    let shift = key_down(0x10); // VK_SHIFT
    let win = key_down(0x5B) || key_down(0x5C); // LWIN / RWIN
    (ctrl, alt, shift, win)
}

fn key_down(vk: i32) -> bool {
    // SAFETY: pure keyboard state query.
    unsafe { (GetKeyState(vk) as u16 & 0x8000) != 0 }
}

unsafe fn paint_picker(hwnd: HWND) {
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

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    let (config, live_preview, font, focused, bg_brush, dirty) = if !ptr.is_null() {
        (
            (*ptr).config.clone(),
            (*ptr).live_preview.clone(),
            (*ptr).font,
            (*ptr).focused,
            (*ptr).bg_brush,
            is_dirty(&*ptr),
        )
    } else {
        (
            String::new(),
            None,
            GetStockObject(windows_sys::Win32::Graphics::Gdi::DEFAULT_GUI_FONT) as HFONT,
            false,
            CreateSolidBrush(LIGHT_EDIT_BG),
            false,
        )
    };

    FillRect(hdc, &rect, bg_brush);

    // Leave room for the Reset button when it is visible (text only; outline wraps all).
    let text_right = if dirty {
        rect.right - RESET_BTN_W - RESET_GAP - 2
    } else {
        rect.right
    };

    // Inner focus outline around the full control (chips + Reset).
    if focused {
        let pen_brush = CreateSolidBrush(FOCUS_BORDER);
        // Draw a 2px inset rectangle via four thin fills.
        let t = 2i32;
        let top = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.top + t,
        };
        let bottom = RECT {
            left: rect.left,
            top: rect.bottom - t,
            right: rect.right,
            bottom: rect.bottom,
        };
        let left = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.left + t,
            bottom: rect.bottom,
        };
        let right = RECT {
            left: rect.right - t,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        FillRect(hdc, &top, pen_brush);
        FillRect(hdc, &bottom, pen_brush);
        FillRect(hdc, &left, pen_brush);
        FillRect(hdc, &right, pen_brush);
        DeleteObject(pen_brush);
    }

    // Prefer live key-down preview; otherwise committed config chips; else focus hint.
    let (display, is_hint) = if let Some(preview) = live_preview.filter(|s| !s.is_empty()) {
        (preview, false)
    } else if !config.is_empty() {
        (format_shortcut_display(&config), false)
    } else if focused {
        (CAPTURE_HINT.to_owned(), true)
    } else {
        (String::new(), false)
    };

    let text_color = if is_hint {
        rgb(120, 120, 120)
    } else {
        LIGHT_TEXT
    };

    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, text_color);
    let old_font = if !font.is_null() {
        SelectObject(hdc, font)
    } else {
        ptr::null_mut()
    };

    let mut wide = to_wide(&display);
    // DrawTextW does not need the trailing NUL in the count.
    let len = wide.len().saturating_sub(1) as i32;
    let mut text_rect = RECT {
        left: rect.left + 8,
        top: rect.top,
        right: text_right - 8,
        bottom: rect.bottom,
    };
    DrawTextW(
        hdc,
        wide.as_mut_ptr(),
        len,
        &mut text_rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    if !old_font.is_null() {
        SelectObject(hdc, old_font);
    }

    // If we created a temporary brush for null state, free it.
    if ptr.is_null() && !bg_brush.is_null() {
        DeleteObject(bg_brush);
    }

    EndPaint(hwnd, &ps);
}

// ── Modal prompt dialog (tray → Change shortcut) ───────────────────────────

struct PromptState {
    font: HFONT,
    header_font: HFONT,
    bg_brush: HBRUSH,
    picker: HWND,
    result: Option<String>,
    cancelled: bool,
}

struct PromptInit {
    initial: String,
}

thread_local! {
    static PROMPT_STATE: RefCell<Option<PromptState>> = const { RefCell::new(None) };
}

unsafe fn prompt_hotkey_dialog_impl(
    title: &str,
    initial: &str,
) -> Result<Option<String>, DialogError> {
    ensure_prompt_class();
    let _visual_styles = activate_visual_styles();
    ensure_common_controls();

    let init = PromptInit {
        initial: initial.to_owned(),
    };
    let class_name = to_wide(PROMPT_CLASS);
    let title_wide = to_wide(title);
    let hinstance = GetModuleHandleW(ptr::null());
    const CLIENT_W: i32 = 420;
    const CLIENT_H: i32 = 190;
    let (outer_w, outer_h) = outer_size_for_client(CLIENT_W, CLIENT_H);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE,
        class_name.as_ptr(),
        title_wide.as_ptr(),
        WINDOW_STYLE | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        outer_w,
        outer_h,
        ptr::null_mut(),
        ptr::null_mut(),
        hinstance,
        &init as *const PromptInit as *const _,
    );
    if hwnd.is_null() {
        return Err(DialogError::Message(
            "failed to create hotkey prompt dialog".into(),
        ));
    }

    apply_titlebar_theme(hwnd, false);
    center_window(hwnd);
    ShowWindow(hwnd, SW_SHOW);

    let mut msg = std::mem::zeroed::<MSG>();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let result = PROMPT_STATE.with(|cell| {
        cell.borrow_mut()
            .take()
            .and_then(|state| if state.cancelled { None } else { state.result })
    });
    Ok(result)
}

fn ensure_prompt_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = to_wide(PROMPT_CLASS);
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(prompt_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: ptr::null_mut(),
            };
            let _ = RegisterClassExW(&class);
        }
    });
}

unsafe extern "system" fn prompt_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = &*(lparam
                as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let init = &*(create.lpCreateParams as *const PromptInit);
            if let Err(error) = build_prompt_ui(hwnd, init) {
                let title = to_wide("Glossa");
                let message = to_wide(&error);
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    hwnd,
                    message.as_ptr(),
                    title.as_ptr(),
                    windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                );
                DestroyWindow(hwnd);
                return -1;
            }
            0
        }
        WM_ERASEBKGND => {
            let brush = PROMPT_STATE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| s.bg_brush)
                    .filter(|b| !b.is_null())
            });
            if let Some(brush) = brush {
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                if GetClientRect(hwnd, &mut rect) != 0 {
                    FillRect(wparam as _, &rect, brush);
                    return 1;
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = wparam as _;
            let brush = PROMPT_STATE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| s.bg_brush)
                    .unwrap_or(ptr::null_mut())
            });
            SetTextColor(hdc, LIGHT_TEXT);
            SetBkMode(hdc, TRANSPARENT as i32);
            brush as LRESULT
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            match id {
                ID_OK => {
                    let outcome = PROMPT_STATE.with(|cell| {
                        cell.borrow().as_ref().map(|state| {
                            let config = hotkey_picker_get(state.picker);
                            if config.trim().is_empty() {
                                Err("Shortcut cannot be empty.".to_owned())
                            } else if let Err(error) = parse_shortcut(&config) {
                                Err(format!("Invalid shortcut:\n{error}"))
                            } else {
                                Ok(config)
                            }
                        })
                    });
                    match outcome {
                        Some(Ok(config)) => {
                            PROMPT_STATE.with(|cell| {
                                if let Some(state) = cell.borrow_mut().as_mut() {
                                    state.result = Some(config);
                                    state.cancelled = false;
                                }
                            });
                            DestroyWindow(hwnd);
                        }
                        Some(Err(message)) => {
                            let title = to_wide("Glossa");
                            let body = to_wide(&message);
                            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                                hwnd,
                                body.as_ptr(),
                                title.as_ptr(),
                                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                            );
                        }
                        None => {}
                    }
                }
                ID_CANCEL => {
                    PROMPT_STATE.with(|cell| {
                        if let Some(state) = cell.borrow_mut().as_mut() {
                            state.cancelled = true;
                            state.result = None;
                        }
                    });
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PROMPT_STATE.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    if !state.font.is_null() {
                        DeleteObject(state.font);
                        state.font = ptr::null_mut();
                    }
                    if !state.header_font.is_null() {
                        DeleteObject(state.header_font);
                        state.header_font = ptr::null_mut();
                    }
                    if !state.bg_brush.is_null() {
                        DeleteObject(state.bg_brush);
                        state.bg_brush = ptr::null_mut();
                    }
                }
            });
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn build_prompt_ui(hwnd: HWND, init: &PromptInit) -> Result<(), String> {
    let hinstance = GetModuleHandleW(ptr::null());
    let font = create_body_font();
    let header_font = create_header_font();
    let bg_brush = CreateSolidBrush(LIGHT_BG);

    const CLIENT_W: i32 = 420;
    let margin = 20i32;
    let field_w = CLIENT_W - margin * 2;
    let mut y = margin;

    let label = CreateWindowExW(
        0,
        to_wide("STATIC").as_ptr(),
        to_wide("Press the new global hotkey:").as_ptr(),
        WS_CHILD | WS_VISIBLE,
        margin,
        y,
        field_w,
        22,
        hwnd,
        ptr::null_mut(),
        hinstance,
        ptr::null(),
    );
    if label.is_null() {
        return Err("failed to create label".into());
    }
    SendMessageW(label, WM_SETFONT, header_font as usize, 1);
    y += 30;

    let hint = CreateWindowExW(
        0,
        to_wide("STATIC").as_ptr(),
        to_wide("Click the field, then press a key combination.").as_ptr(),
        WS_CHILD | WS_VISIBLE,
        margin,
        y,
        field_w,
        18,
        hwnd,
        ptr::null_mut(),
        hinstance,
        ptr::null(),
    );
    if !hint.is_null() {
        SendMessageW(hint, WM_SETFONT, font as usize, 1);
    }
    y += 26;

    let picker = create_hotkey_picker(
        hwnd,
        margin,
        y,
        field_w,
        32,
        ID_PICKER,
        font,
        &init.initial,
    );
    if picker.is_null() {
        return Err("failed to create hotkey picker".into());
    }
    y += 44;

    const SS_ETCHEDHORZ: u32 = 0x10;
    let _ = CreateWindowExW(
        0,
        to_wide("STATIC").as_ptr(),
        to_wide("").as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
        margin,
        y,
        field_w,
        2,
        hwnd,
        ptr::null_mut(),
        hinstance,
        ptr::null(),
    );
    y += 14;

    let btn_w = 96i32;
    let btn_h = 32i32;
    let ok = CreateWindowExW(
        0,
        to_wide("BUTTON").as_ptr(),
        to_wide("OK").as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        CLIENT_W - margin - btn_w,
        y,
        btn_w,
        btn_h,
        hwnd,
        ID_OK as _,
        hinstance,
        ptr::null(),
    );
    let cancel = CreateWindowExW(
        0,
        to_wide("BUTTON").as_ptr(),
        to_wide("Cancel").as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        CLIENT_W - margin - btn_w * 2 - 10,
        y,
        btn_w,
        btn_h,
        hwnd,
        ID_CANCEL as _,
        hinstance,
        ptr::null(),
    );
    if ok.is_null() || cancel.is_null() {
        return Err("failed to create buttons".into());
    }
    SendMessageW(ok, WM_SETFONT, font as usize, 1);
    SendMessageW(cancel, WM_SETFONT, font as usize, 1);

    let content_bottom = y + btn_h + margin;
    fit_window_to_client(hwnd, CLIENT_W, content_bottom);

    PROMPT_STATE.with(|cell| {
        *cell.borrow_mut() = Some(PromptState {
            font,
            header_font,
            bg_brush,
            picker,
            result: None,
            cancelled: true,
        });
    });
    SetFocus(picker);
    Ok(())
}
