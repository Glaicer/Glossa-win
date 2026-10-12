/// Visual state of the on-screen indicator pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillMode {
    Recording,
    Processing,
}

pub(crate) const PILL_WIDTH: i32 = 320;
pub(crate) const PILL_HEIGHT: i32 = 72;
const PILL_RADIUS: f64 = 36.0;
const BOTTOM_MARGIN: i32 = 28;

const MIC_SIZE: f64 = 32.0;
const MIC_ORIGIN_X: f64 = 24.0;

const BAR_COUNT: usize = 10;
const BAR_WIDTH: f64 = 8.0;
const BAR_GAP: f64 = 15.0;
const BAR_MIN_HEIGHT: f64 = 6.0;
const BAR_MAX_HEIGHT: f64 = 40.0;
const BAR_ORIGIN_X: f64 = MIC_ORIGIN_X + MIC_SIZE + 20.0;
const BAR_WOBBLE_SPEED: f64 = 9.0;
const MIN_BAR_FRACTION: f64 = BAR_MIN_HEIGHT / BAR_MAX_HEIGHT;
const BAR_MULTIPLIERS: [f64; BAR_COUNT] =
    [0.30, 0.48, 0.66, 0.84, 1.0, 1.0, 0.84, 0.66, 0.48, 0.30];

const DOT_MIN_RADIUS: f64 = 3.0;
const DOT_MAX_RADIUS: f64 = 8.0;

const LEVEL_ATTACK: f32 = 0.45;
const LEVEL_DECAY: f32 = 0.10;

/// Moves the displayed level toward the microphone level: quick on attack,
/// slow release so the waveform trails off instead of snapping to rest.
pub fn smooth_level(current: f32, target: f32) -> f32 {
    let factor = if target > current {
        LEVEL_ATTACK
    } else {
        LEVEL_DECAY
    };
    (current + (target - current) * factor).clamp(0.0, 1.0)
}

/// Height of one waveform bar as a fraction of the maximum bar height.
pub fn bar_fraction(level: f32, elapsed_secs: f64, index: usize) -> f64 {
    let multiplier = BAR_MULTIPLIERS
        .get(index)
        .copied()
        .expect("bar index is inside the waveform");
    let wobble = 0.88 + 0.12 * (elapsed_secs * BAR_WOBBLE_SPEED + index as f64 * 0.9).sin();
    (f64::from(level) * multiplier * wobble).clamp(MIN_BAR_FRACTION, 1.0)
}

/// Radius of one processing dot.
pub fn dot_radius(elapsed_secs: f64, index: usize) -> f64 {
    let phase = index as f64 * 1.7;
    let pulse = 0.5
        + 0.5
            * (0.65 * (elapsed_secs * 4.0 + phase).sin()
                + 0.35 * (elapsed_secs * 2.7 + phase * 1.3).sin());
    DOT_MIN_RADIUS + (DOT_MAX_RADIUS - DOT_MIN_RADIUS) * pulse
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{
        HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BLENDFUNCTION, HBITMAP, HDC, HGDIOBJ,
    };
    use windows_sys::Win32::Graphics::GdiPlus::{
        CompositingModeSourceOver, FillMode, FillModeAlternate, FillModeWinding, GdipAddPathArc,
        GdipAddPathBezier, GdipAddPathEllipse, GdipAddPathLine, GdipClosePathFigure,
        GdipCreateBitmapFromScan0, GdipCreateHBITMAPFromBitmap, GdipCreateMatrix2, GdipCreatePath,
        GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeleteMatrix, GdipDeletePath,
        GdipDisposeImage, GdipFillEllipse, GdipFillPath, GdipGetImageGraphicsContext,
        GdipGraphicsClear, GdipResetWorldTransform, GdipSetCompositingMode, GdipSetSmoothingMode,
        GdipSetWorldTransform, GdipStartPathFigure, GdiplusShutdown, GdiplusStartup,
        GdiplusStartupInput, GpBitmap, GpBrush, GpGraphics, GpImage, GpPath, GpSolidFill, Matrix,
        SmoothingModeAntiAlias,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
        IsWindow, PeekMessageW, PostQuitMessage, RegisterClassW, SetWindowPos, ShowWindow,
        SystemParametersInfoW, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, PM_REMOVE,
        SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{
        bar_fraction, dot_radius, smooth_level, PillMode, BAR_COUNT, BAR_GAP, BAR_MAX_HEIGHT,
        BAR_MIN_HEIGHT, BAR_ORIGIN_X, BAR_WIDTH, BOTTOM_MARGIN, MIC_ORIGIN_X, MIC_SIZE,
        PILL_HEIGHT, PILL_RADIUS, PILL_WIDTH,
    };

    // windows-sys does not bind the GDI+ pixel-format constant or the status value.
    const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x0026_200A;
    const GDIPLUS_OK: i32 = 0;

    /// Initializes GDI+ for the calling thread and shuts it down on drop.
    pub(crate) struct GdiPlusGuard {
        token: usize,
    }

    impl GdiPlusGuard {
        pub(crate) fn new() -> Result<Self, String> {
            let mut token: usize = 0;
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: 0,
                SuppressExternalCodecs: 0,
            };
            // SAFETY: input points at a valid stack struct; token receives the startup token.
            let status = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()) };
            if status != GDIPLUS_OK {
                return Err(format!("GdiplusStartup failed with status {status}"));
            }
            Ok(Self { token })
        }
    }

    impl Drop for GdiPlusGuard {
        fn drop(&mut self) {
            // SAFETY: token was produced by a matching GdiplusStartup on this thread.
            unsafe { GdiplusShutdown(self.token) };
        }
    }

    /// Layered, click-through, always-on-top pill window drawn with GDI+.
    pub(crate) struct PillWindow {
        hwnd: HWND,
        mode: PillMode,
        level: f32,
        target: f32,
        started_at: Instant,
    }

    impl PillWindow {
        pub(crate) fn new(mode: PillMode) -> Result<Self, String> {
            // SAFETY: class/name are NUL-terminated wide strings and the wndproc is static.
            unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                register_class(hinstance)?;
                let class_name = wide("glossa-recording-pill");
                let window_name = wide("glossa-recording");
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TRANSPARENT
                        | WS_EX_TOPMOST
                        | WS_EX_TOOLWINDOW
                        | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    window_name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    PILL_WIDTH,
                    PILL_HEIGHT,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null_mut(),
                );
                if hwnd.is_null() {
                    return Err("failed to create the overlay window".into());
                }
                position_bottom_center(hwnd);
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                Ok(Self {
                    hwnd,
                    mode,
                    level: 0.0,
                    target: 0.0,
                    started_at: Instant::now(),
                })
            }
        }

        pub(crate) fn set_mode(&mut self, mode: PillMode) {
            self.mode = mode;
        }

        /// Sets the microphone level (0..=100) the waveform follows.
        pub(crate) fn set_target(&mut self, percent: u8) {
            self.target = f32::from(percent.min(100)) / 100.0;
        }

        /// Renders one frame and pumps the window message queue.
        pub(crate) fn render_frame(&mut self) {
            self.level = smooth_level(self.level, self.target);
            let elapsed_secs = self.started_at.elapsed().as_secs_f64();
            // SAFETY: hwnd is a live window owned by this thread and GDI+ is initialized.
            unsafe { compose_frame(self.hwnd, self.mode, self.level, elapsed_secs) };
            // SAFETY: the message pump runs on the thread that owns the window.
            unsafe { pump_messages() };
        }

        pub(crate) fn is_alive(&self) -> bool {
            // SAFETY: hwnd is either null (already destroyed) or a live window handle.
            unsafe { IsWindow(self.hwnd) != 0 }
        }
    }

    impl Drop for PillWindow {
        fn drop(&mut self) {
            // SAFETY: the window is owned by the thread running this drop.
            unsafe {
                if IsWindow(self.hwnd) != 0 {
                    DestroyWindow(self.hwnd);
                }
            }
        }
    }

    unsafe fn register_class(hinstance: HINSTANCE) -> Result<(), String> {
        use std::sync::Once;

        static REGISTER_CLASS: Once = Once::new();
        let mut failure: Option<String> = None;
        REGISTER_CLASS.call_once(|| {
            let class_name = wide("glossa-recording-pill");
            let wndclass = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(pill_wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            // SAFETY: wndclass points at a fully initialized stack struct.
            if RegisterClassW(&wndclass) == 0 {
                failure = Some("failed to register the overlay window class".into());
            }
        });
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    unsafe extern "system" fn pill_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
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
        let x = left + (width - PILL_WIDTH) / 2;
        let y = bottom - PILL_HEIGHT - BOTTOM_MARGIN;
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            PILL_WIDTH,
            PILL_HEIGHT,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    unsafe fn pump_messages() {
        let mut message = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    unsafe fn compose_frame(hwnd: HWND, mode: PillMode, level: f32, elapsed_secs: f64) {
        let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
        if GdipCreateBitmapFromScan0(
            PILL_WIDTH,
            PILL_HEIGHT,
            0,
            PIXEL_FORMAT_32BPP_PARGB,
            std::ptr::null_mut(),
            &mut bitmap,
        ) != GDIPLUS_OK
            || bitmap.is_null()
        {
            return;
        }
        let mut graphics: *mut GpGraphics = std::ptr::null_mut();
        let image = bitmap.cast::<GpImage>();
        if GdipGetImageGraphicsContext(image, &mut graphics) != GDIPLUS_OK || graphics.is_null() {
            GdipDisposeImage(image);
            return;
        }
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        GdipSetCompositingMode(graphics, CompositingModeSourceOver);
        draw_pill(graphics, mode, level, elapsed_secs);
        GdipDeleteGraphics(graphics);

        let mut hbitmap: HBITMAP = std::ptr::null_mut();
        if GdipCreateHBITMAPFromBitmap(bitmap, &mut hbitmap, 0) != GDIPLUS_OK || hbitmap.is_null() {
            GdipDisposeImage(image);
            return;
        }
        GdipDisposeImage(image);

        let screen_dc: HDC = GetDC(std::ptr::null_mut());
        let memory_dc = CreateCompatibleDC(screen_dc);
        let previous: HGDIOBJ = SelectObject(memory_dc, hbitmap);
        let size = SIZE {
            cx: PILL_WIDTH,
            cy: PILL_HEIGHT,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // Best effort: a failed frame is simply retried on the next tick.
        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            std::ptr::null(),
            &size,
            memory_dc,
            &source,
            0,
            &blend,
            ULW_ALPHA,
        );
        SelectObject(memory_dc, previous);
        DeleteObject(hbitmap);
        DeleteDC(memory_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
    }

    unsafe fn draw_pill(graphics: *mut GpGraphics, mode: PillMode, level: f32, elapsed_secs: f64) {
        GdipGraphicsClear(graphics, 0x0000_0000);
        let background = brush(0xFF40_4040);
        let path = new_path(FillModeWinding);
        add_rounded_rect(
            path,
            0.0,
            0.0,
            f64::from(PILL_WIDTH),
            f64::from(PILL_HEIGHT),
            PILL_RADIUS,
        );
        fill_path(graphics, background, path);
        GdipDeletePath(path);
        GdipDeleteBrush(background);

        match mode {
            PillMode::Recording => {
                draw_microphone(graphics);
                draw_bars(graphics, level, elapsed_secs);
            }
            PillMode::Processing => {
                draw_server(graphics);
                draw_processing_dots(graphics, elapsed_secs);
            }
        }
    }

    unsafe fn draw_microphone(graphics: *mut GpGraphics) {
        // The microphone glyph is laid out on a 24x24 grid scaled to MIC_SIZE,
        // matching the SVG path data used by the Linux version.
        let scale = (MIC_SIZE / 24.0) as f32;
        let origin_y = ((f64::from(PILL_HEIGHT) - MIC_SIZE) / 2.0) as f32;

        let mut matrix: *mut Matrix = std::ptr::null_mut();
        expect_status(
            GdipCreateMatrix2(
                scale,
                0.0,
                0.0,
                scale,
                MIC_ORIGIN_X as f32,
                origin_y,
                &mut matrix,
            ),
            "create microphone transform",
        );
        expect_status(
            GdipSetWorldTransform(graphics, matrix),
            "set microphone transform",
        );
        GdipDeleteMatrix(matrix);

        let white = brush(0xFFFF_FFFF);

        // Head: rounded capsule body with domed top (exact SVG path, 24x24 grid).
        let head = new_path(FillModeWinding);
        GdipAddPathBezier(head, 12.0, 16.0, 14.206, 16.0, 16.0, 14.206, 16.0, 12.0);
        GdipAddPathLine(head, 16.0, 12.0, 16.0, 6.0);
        GdipAddPathBezier(head, 16.0, 6.0, 16.0, 3.783, 14.215, 1.979, 12.021, 1.979);
        GdipAddPathLine(head, 12.021, 1.979, 11.812, 2.004);
        GdipAddPathArc(head, 7.994, 1.994, 8.012, 8.012, 267.3, -87.3);
        GdipAddPathLine(head, 8.0, 6.0, 8.0, 12.0);
        GdipAddPathBezier(head, 8.0, 12.0, 8.0, 14.206, 9.794, 16.0, 12.0, 16.0);
        GdipClosePathFigure(head);
        fill_path(graphics, white, head);
        GdipDeletePath(head);

        // Stand: U-shaped band with the stem (exact SVG path).
        let stand = new_path(FillModeWinding);
        GdipAddPathLine(stand, 11.0, 19.931, 11.0, 22.0);
        GdipAddPathLine(stand, 11.0, 22.0, 13.0, 22.0);
        GdipAddPathLine(stand, 13.0, 22.0, 13.0, 19.931);
        GdipAddPathBezier(
            stand, 13.0, 19.931, 16.939, 19.436, 20.0, 16.073, 20.0, 12.0,
        );
        GdipAddPathLine(stand, 20.0, 12.0, 18.0, 12.0);
        GdipAddPathBezier(stand, 18.0, 12.0, 18.0, 15.309, 15.309, 18.0, 12.0, 18.0);
        GdipAddPathBezier(stand, 12.0, 18.0, 8.691, 18.0, 6.0, 15.309, 6.0, 12.0);
        GdipAddPathLine(stand, 6.0, 12.0, 4.0, 12.0);
        GdipAddPathBezier(stand, 4.0, 12.0, 4.0, 16.072, 7.061, 19.436, 11.0, 19.931);
        GdipClosePathFigure(stand);
        fill_path(graphics, white, stand);
        GdipDeletePath(stand);

        GdipDeleteBrush(white);
        expect_status(
            GdipResetWorldTransform(graphics),
            "reset microphone transform",
        );
    }

    unsafe fn draw_bars(graphics: *mut GpGraphics, level: f32, elapsed_secs: f64) {
        let white = brush(0xFFFF_FFFF);
        let path = new_path(FillModeWinding);
        for index in 0..BAR_COUNT {
            let fraction = bar_fraction(level, elapsed_secs, index);
            let height = BAR_MIN_HEIGHT + (BAR_MAX_HEIGHT - BAR_MIN_HEIGHT) * fraction;
            let x = BAR_ORIGIN_X + index as f64 * (BAR_WIDTH + BAR_GAP);
            let y = (f64::from(PILL_HEIGHT) - height) / 2.0;
            add_rounded_rect(path, x, y, BAR_WIDTH, height, BAR_WIDTH / 2.0);
        }
        fill_path(graphics, white, path);
        GdipDeletePath(path);
        GdipDeleteBrush(white);
    }

    unsafe fn draw_server(graphics: *mut GpGraphics) {
        // The server-rack glyph is laid out on a 512x512 grid scaled to MIC_SIZE.
        let scale = MIC_SIZE / 512.0;
        let origin_y = (f64::from(PILL_HEIGHT) - MIC_SIZE) / 2.0;

        let white = brush(0xFFFF_FFFF);
        let path = new_path(FillModeAlternate);
        for row in [32.0, 192.0, 352.0] {
            add_rounded_rect(
                path,
                MIC_ORIGIN_X,
                origin_y + row * scale,
                512.0 * scale,
                128.0 * scale,
                32.0 * scale,
            );
            for hole_x in [368.0, 432.0] {
                let center = (
                    MIC_ORIGIN_X + hole_x * scale,
                    origin_y + (row + 64.0) * scale,
                );
                let radius = 24.0 * scale;
                GdipAddPathEllipse(
                    path,
                    (center.0 - radius) as f32,
                    (center.1 - radius) as f32,
                    (radius * 2.0) as f32,
                    (radius * 2.0) as f32,
                );
            }
        }
        fill_path(graphics, white, path);
        GdipDeletePath(path);
        GdipDeleteBrush(white);
    }

    unsafe fn draw_processing_dots(graphics: *mut GpGraphics, elapsed_secs: f64) {
        let white = brush(0xFFFF_FFFF);
        let center_y = f64::from(PILL_HEIGHT) / 2.0;
        for index in 0..BAR_COUNT {
            let x = BAR_ORIGIN_X + BAR_WIDTH / 2.0 + index as f64 * (BAR_WIDTH + BAR_GAP);
            let radius = dot_radius(elapsed_secs, index);
            GdipFillEllipse(
                graphics,
                white,
                (x - radius) as f32,
                (center_y - radius) as f32,
                (radius * 2.0) as f32,
                (radius * 2.0) as f32,
            );
        }
        GdipDeleteBrush(white);
    }

    unsafe fn brush(argb: u32) -> *mut GpBrush {
        let mut fill: *mut GpSolidFill = std::ptr::null_mut();
        expect_status(GdipCreateSolidFill(argb, &mut fill), "create solid brush");
        fill.cast()
    }

    unsafe fn new_path(fill_mode: FillMode) -> *mut GpPath {
        let mut path: *mut GpPath = std::ptr::null_mut();
        expect_status(GdipCreatePath(fill_mode, &mut path), "create path");
        path
    }

    unsafe fn fill_path(graphics: *mut GpGraphics, brush: *mut GpBrush, path: *mut GpPath) {
        expect_status(GdipFillPath(graphics, brush, path), "fill path");
    }

    unsafe fn expect_status(status: i32, what: &'static str) {
        if status != GDIPLUS_OK {
            panic!("{what} failed with GDI+ status {status}");
        }
    }

    unsafe fn add_rounded_rect(
        path: *mut GpPath,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        radius: f64,
    ) {
        let radius = radius.min(width / 2.0).min(height / 2.0) as f32;
        let (x, y, width, height) = (x as f32, y as f32, width as f32, height as f32);
        let diameter = radius * 2.0;
        GdipStartPathFigure(path);
        GdipAddPathArc(path, x, y, diameter, diameter, 180.0, 90.0);
        GdipAddPathArc(
            path,
            x + width - diameter,
            y,
            diameter,
            diameter,
            270.0,
            90.0,
        );
        GdipAddPathArc(
            path,
            x + width - diameter,
            y + height - diameter,
            diameter,
            diameter,
            0.0,
            90.0,
        );
        GdipAddPathArc(
            path,
            x,
            y + height - diameter,
            diameter,
            diameter,
            90.0,
            90.0,
        );
        GdipClosePathFigure(path);
    }

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    #[cfg(test)]
    mod render_tests {
        use std::sync::{Mutex, OnceLock};

        use windows_sys::Win32::Graphics::GdiPlus::{GdipBitmapGetPixel, GpImage};

        use super::{
            draw_pill, CompositingModeSourceOver, GdipCreateBitmapFromScan0, GdipDeleteGraphics,
            GdipDisposeImage, GdipGetImageGraphicsContext, GdipSetCompositingMode,
            GdipSetSmoothingMode, GpBitmap, GpGraphics, SmoothingModeAntiAlias, GDIPLUS_OK,
            PILL_HEIGHT, PILL_WIDTH, PIXEL_FORMAT_32BPP_PARGB,
        };

        use super::super::{PillMode, BAR_COUNT};

        /// Serializes GDI+ render tests; concurrent GdiplusShutdown would
        /// tear down the library while another test is still drawing.
        fn gdiplus_lock() -> std::sync::MutexGuard<'static, ()> {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            LOCK.get_or_init(|| Mutex::new(()))
                .lock()
                .expect("gdiplus render test lock")
        }

        struct Canvas {
            _gdiplus: super::GdiPlusGuard,
            bitmap: *mut GpBitmap,
        }

        impl Canvas {
            fn new(mode: PillMode, level: f32, elapsed_secs: f64) -> Self {
                let _gdiplus = super::GdiPlusGuard::new().expect("gdiplus startup");
                let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
                // SAFETY: output pointer receives a fresh GDI+ bitmap.
                let status = unsafe {
                    GdipCreateBitmapFromScan0(
                        PILL_WIDTH,
                        PILL_HEIGHT,
                        0,
                        PIXEL_FORMAT_32BPP_PARGB,
                        std::ptr::null_mut(),
                        &mut bitmap,
                    )
                };
                assert_eq!(status, GDIPLUS_OK, "create test bitmap");
                let mut graphics: *mut GpGraphics = std::ptr::null_mut();
                // SAFETY: bitmap was just created for this test.
                unsafe {
                    let status =
                        GdipGetImageGraphicsContext(bitmap.cast::<GpImage>(), &mut graphics);
                    assert_eq!(status, GDIPLUS_OK, "create test graphics");
                    GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
                    GdipSetCompositingMode(graphics, CompositingModeSourceOver);
                    draw_pill(graphics, mode, level, elapsed_secs);
                    GdipDeleteGraphics(graphics);
                }
                Self { _gdiplus, bitmap }
            }

            fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8, u8) {
                let mut color: u32 = 0;
                // SAFETY: bitmap is a live GDI+ bitmap owned by this test.
                let status = unsafe { GdipBitmapGetPixel(self.bitmap, x, y, &mut color) };
                assert_eq!(status, GDIPLUS_OK, "read test pixel");
                (
                    ((color >> 24) & 0xFF) as u8,
                    ((color >> 16) & 0xFF) as u8,
                    ((color >> 8) & 0xFF) as u8,
                    (color & 0xFF) as u8,
                )
            }
        }

        impl Drop for Canvas {
            fn drop(&mut self) {
                // SAFETY: bitmap is a live GDI+ bitmap owned by this test.
                unsafe { GdipDisposeImage(self.bitmap.cast::<GpImage>()) };
            }
        }

        const BACKGROUND: (u8, u8, u8, u8) = (255, 64, 64, 64);
        const WHITE: (u8, u8, u8, u8) = (255, 255, 255, 255);
        const TRANSPARENT: (u8, u8, u8, u8) = (0, 0, 0, 0);

        #[test]
        fn rendered_recording_pill_should_draw_dark_background_and_white_glyphs() {
            let _lock = gdiplus_lock();
            let canvas = Canvas::new(PillMode::Recording, 0.5, 1.0);

            assert_eq!(canvas.pixel(2, 2), TRANSPARENT);
            assert_eq!(canvas.pixel(160, 2), BACKGROUND);

            let has_white = (24..56).any(|x| (20..52).any(|y| canvas.pixel(x, y) == WHITE));
            assert!(has_white, "microphone area should contain white pixels");
        }

        #[test]
        fn rendered_processing_pill_should_draw_server_and_dots_instead_of_waveform() {
            let _lock = gdiplus_lock();
            let canvas = Canvas::new(PillMode::Processing, 1.0, 0.0);

            assert_eq!(canvas.pixel(2, 2), TRANSPARENT);
            assert_eq!(canvas.pixel(160, 2), BACKGROUND);

            for y in [26, 36, 46] {
                assert_eq!(canvas.pixel(30, y), WHITE, "server row at y={y}");
            }
            assert_eq!(canvas.pixel(30, 31), BACKGROUND, "server hole");
            assert_eq!(canvas.pixel(47, 26), BACKGROUND, "server hole");
            for index in 0..BAR_COUNT as i32 {
                assert_eq!(
                    canvas.pixel(80 + index * 23, 36),
                    WHITE,
                    "processing dot {index}"
                );
                assert_eq!(
                    canvas.pixel(80 + index * 23, 24),
                    BACKGROUND,
                    "above processing dot {index}"
                );
            }
        }

        // shortcut: manual visual check; run with `cargo test -- --ignored`
        #[test]
        #[ignore = "writes preview PNGs to the temp directory"]
        fn render_pill_previews_to_temp() {
            let _lock = gdiplus_lock();
            let dir = std::env::temp_dir().join("glossa-pill-preview");
            std::fs::create_dir_all(&dir).expect("create preview dir");
            for (name, mode, level, elapsed) in [
                ("recording", PillMode::Recording, 0.7_f32, 1.0_f64),
                ("processing", PillMode::Processing, 0.0, 0.8),
            ] {
                let canvas = Canvas::new(mode, level, elapsed);
                let mut data = Vec::with_capacity((PILL_WIDTH * PILL_HEIGHT * 4) as usize);
                for y in 0..PILL_HEIGHT {
                    for x in 0..PILL_WIDTH {
                        let (r, g, b, a) = canvas.pixel(x, y);
                        data.extend_from_slice(&[r, g, b, a]);
                    }
                }
                let file = std::fs::File::create(dir.join(format!("{name}.png")))
                    .expect("create preview file");
                let mut encoder = png::Encoder::new(file, PILL_WIDTH as u32, PILL_HEIGHT as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("png header");
                writer.write_image_data(&data).expect("png data");
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use windows_impl::{GdiPlusGuard, PillWindow};

#[cfg(test)]
mod tests {
    use super::{
        bar_fraction, dot_radius, smooth_level, DOT_MAX_RADIUS, DOT_MIN_RADIUS, MIN_BAR_FRACTION,
    };

    #[test]
    fn silence_should_rest_at_minimum_bars() {
        for index in 0..10 {
            assert_eq!(bar_fraction(0.0, 1.7, index), MIN_BAR_FRACTION);
        }
    }

    #[test]
    fn center_bars_should_be_taller_than_outer_bars() {
        let center = bar_fraction(1.0, 0.0, 4);
        let outer = bar_fraction(1.0, 0.0, 0);
        assert!(center > outer);
        assert!(center <= 1.0);
    }

    #[test]
    fn louder_level_should_produce_taller_bars() {
        assert!(bar_fraction(0.9, 0.1, 4) > bar_fraction(0.3, 0.1, 4));
    }

    #[test]
    fn rising_level_should_be_followed_quickly() {
        let next = smooth_level(0.2, 0.8);
        assert!(next > 0.4);
        assert!(next < 0.8);
    }

    #[test]
    fn falling_level_should_release_slowly() {
        let next = smooth_level(0.8, 0.0);
        assert!(next > 0.7);
        assert!(next < 0.8);
    }

    #[test]
    fn level_should_stay_within_bounds() {
        assert_eq!(smooth_level(0.0, 0.0), 0.0);
        assert_eq!(smooth_level(1.0, 1.0), 1.0);
    }

    #[test]
    fn processing_dots_should_pulse_independently_within_bounds() {
        for index in 0..10 {
            for frame in 0..600 {
                let radius = dot_radius(f64::from(frame) / 60.0, index);
                assert!((DOT_MIN_RADIUS..=DOT_MAX_RADIUS).contains(&radius));
            }
            assert_ne!(dot_radius(0.0, index), dot_radius(0.3, index));
        }
        assert_ne!(dot_radius(0.3, 0), dot_radius(0.3, 1));
    }
}
