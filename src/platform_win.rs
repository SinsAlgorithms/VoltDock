use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleVisible,
    ShowAndFocus,
    OpenScanner,
    RefreshNow,
    TogglePin,
    ToggleCompact,
    Quit,
}

#[derive(Debug, Clone)]
pub struct TraySyncState {
    pub visible: bool,
    pub always_on_top: bool,
    pub compact_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectI32 {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectI32 {
    pub fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }

    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorX {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorY {
    Top,
    Bottom,
}

/// Determines which horizontal and vertical edges of the widget are closer to the monitor's
/// work area edges so resizing expands away from the nearest screen boundary.
pub fn infer_anchors(rect: RectI32, work_area: RectI32) -> (AnchorX, AnchorY) {
    let dist_left = (rect.left - work_area.left).max(0);
    let dist_right = (work_area.right - rect.right).max(0);
    let anchor_x = if dist_right < dist_left {
        AnchorX::Right
    } else {
        AnchorX::Left
    };

    let dist_top = (rect.top - work_area.top).max(0);
    let dist_bottom = (work_area.bottom - rect.bottom).max(0);
    let anchor_y = if dist_bottom < dist_top {
        AnchorY::Bottom
    } else {
        AnchorY::Top
    };

    (anchor_x, anchor_y)
}

/// Computes the target window rectangle in physical screen coordinates while preserving
/// the anchored edges and clamping strictly inside `work_area`.
pub fn compute_anchored_rect(
    current: RectI32,
    work_area: RectI32,
    target_w: i32,
    target_h: i32,
    anchor_x: AnchorX,
    anchor_y: AnchorY,
) -> RectI32 {
    let max_w = work_area.width().max(120);
    let max_h = work_area.height().max(40);
    let eff_w = target_w.clamp(120, max_w);
    let eff_h = target_h.clamp(40, max_h);

    let cand_left = match anchor_x {
        AnchorX::Left => {
            if current.left + eff_w > work_area.right {
                work_area.right - eff_w
            } else {
                current.left
            }
        }
        AnchorX::Right => {
            let left = current.right - eff_w;
            if left < work_area.left {
                work_area.left
            } else {
                left
            }
        }
    };
    let max_left = (work_area.right - eff_w).max(work_area.left);
    let new_left = cand_left.clamp(work_area.left, max_left);

    let cand_top = match anchor_y {
        AnchorY::Top => {
            if current.top + eff_h > work_area.bottom {
                work_area.bottom - eff_h
            } else {
                current.top
            }
        }
        AnchorY::Bottom => {
            let top = current.bottom - eff_h;
            if top < work_area.top {
                work_area.top
            } else {
                top
            }
        }
    };
    let max_top = (work_area.bottom - eff_h).max(work_area.top);
    let new_top = cand_top.clamp(work_area.top, max_top);

    RectI32 {
        left: new_left,
        top: new_top,
        right: new_left + eff_w,
        bottom: new_top + eff_h,
    }
}

pub fn hwnd_from_gpui_window(window: &gpui_kit::Window) -> Option<isize> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
        POINT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Dwm::{
        DWMWCP_ROUND, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateDIBSection,
        DIB_RGB_COLORS, DeleteObject, GetDC, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
        MONITORINFO, MonitorFromPoint, MonitorFromWindow, ReleaseDC,
    };
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DispatchMessageW, FindWindowW, GetClientRect, GetCursorPos,
        GetMessageW, GetWindowRect, HICON, HWND_NOTOPMOST, HWND_TOPMOST,
        ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
        PostQuitMessage, RegisterClassW,
        SPI_GETWORKAREA, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
        ShowWindow, SystemParametersInfoW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
        WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    const TRAY_CLASS_NAME: PCWSTR = w!("BatteryDisplayTrayClass");
    const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
    const WM_TRAY_SYNC: u32 = WM_APP + 2;
    const WM_TRAY_BALLOON: u32 = WM_APP + 3;
    const WM_TRAY_QUIT: u32 = WM_APP + 4;
    const WM_TRAY_WAKE: u32 = WM_APP + 5;

    const CMD_TOGGLE_VISIBLE: usize = 1001;
    const CMD_REFRESH: usize = 1002;
    const CMD_TOGGLE_PIN: usize = 1003;
    const CMD_TOGGLE_COMPACT: usize = 1004;
    const CMD_QUIT: usize = 1005;
    const CMD_OPEN_SCANNER: usize = 1006;

    pub fn attach_parent_console() {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    pub fn write_cli_output(text: &str) {
        use std::io::Write as _;
        if std::io::stdout().write_all(text.as_bytes()).is_ok()
            && std::io::stdout().flush().is_ok()
        {
            return;
        }
        if let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = con.write_all(text.as_bytes());
            let _ = con.flush();
        }
    }

    pub struct SingleInstanceGuard {
        handle: HANDLE,
        pub is_primary: bool,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.handle.is_invalid() {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }

    impl SingleInstanceGuard {
        pub fn acquire() -> Self {
            unsafe {
                match CreateMutexW(None, false, w!("Local\\BatteryDisplayWidgetMutex")) {
                    Ok(h) => {
                        let already = GetLastError() == ERROR_ALREADY_EXISTS;
                        Self {
                            handle: h,
                            is_primary: !already,
                        }
                    }
                    Err(_) => Self {
                        handle: HANDLE::default(),
                        is_primary: true,
                    },
                }
            }
        }
    }

    /// Sends a wake-up message to an already-running instance's tray window so relaunching the
    /// executable brings the existing widget back instead of silently exiting.
    pub fn wake_existing_instance() -> bool {
        unsafe {
            if let Ok(hwnd) = FindWindowW(TRAY_CLASS_NAME, PCWSTR::null())
                && !hwnd.is_invalid()
            {
                return PostMessageW(Some(hwnd), WM_TRAY_WAKE, WPARAM(0), LPARAM(0)).is_ok();
            }
        }
        false
    }

    #[inline]
    fn to_hwnd(raw: isize) -> HWND {
        HWND(raw as *mut c_void)
    }

    #[inline]
    fn rect_to_i32(r: RECT) -> RectI32 {
        RectI32 {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }

    pub fn apply_window_style(hwnd_raw: isize, always_on_top: bool) {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            let corner = DWMWCP_ROUND.0 as u32;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
            let insert_after = if always_on_top {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            let _ = SetWindowPos(
                hwnd,
                Some(insert_after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    pub fn get_window_rect(hwnd_raw: isize) -> Option<RectI32> {
        let hwnd = to_hwnd(hwnd_raw);
        let mut rect = RECT::default();
        unsafe {
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                Some(rect_to_i32(rect))
            } else {
                None
            }
        }
    }

    pub fn get_window_frame_insets(hwnd_raw: isize) -> (i32, i32) {
        let hwnd = to_hwnd(hwnd_raw);
        let mut win_rect = RECT::default();
        let mut client_rect = RECT::default();
        unsafe {
            if GetWindowRect(hwnd, &mut win_rect).is_ok()
                && GetClientRect(hwnd, &mut client_rect).is_ok()
            {
                let dw = ((win_rect.right - win_rect.left) - (client_rect.right - client_rect.left))
                    .max(0);
                let dh = ((win_rect.bottom - win_rect.top) - (client_rect.bottom - client_rect.top))
                    .max(0);
                return (dw, dh);
            }
        }
        (0, 0)
    }

    pub fn get_monitor_work_area(hwnd_raw: isize) -> Option<RectI32> {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if hmon.is_invalid() {
                return None;
            }
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                Some(rect_to_i32(mi.rcWork))
            } else {
                None
            }
        }
    }

    pub fn get_monitor_work_area_for_point(x: i32, y: i32) -> RectI32 {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            if !hmon.is_invalid() {
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                    return rect_to_i32(mi.rcWork);
                }
            }
            let mut wa = RECT::default();
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as *mut c_void),
                Default::default(),
            )
            .is_ok()
            {
                return rect_to_i32(wa);
            }
        }
        RectI32::from_xywh(0, 0, 1920, 1040)
    }

    pub fn get_scale_factor_for_point(x: i32, y: i32) -> f32 {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            if !hmon.is_invalid() {
                let mut dpix = 96u32;
                let mut dpiy = 96u32;
                if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpix, &mut dpiy).is_ok()
                    && dpix >= 72
                {
                    return (dpix as f32 / 96.0).clamp(1.0, 4.0);
                }
            }
            let sys_dpi = GetDpiForSystem();
            if sys_dpi >= 72 {
                return (sys_dpi as f32 / 96.0).clamp(1.0, 4.0);
            }
        }
        1.0
    }

    pub fn set_window_bounds(hwnd_raw: isize, x: i32, y: i32, w: i32, h: i32) {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                x,
                y,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn set_window_visible(hwnd_raw: isize, visible: bool) {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    pub fn set_window_visible_and_focus(hwnd_raw: isize, visible: bool, always_on_top: bool) {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
            if visible {
                apply_window_style(hwnd_raw, always_on_top);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    pub fn default_widget_position(width: i32, height: i32) -> (i32, i32) {
        let mut wa = RECT::default();
        unsafe {
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as *mut c_void),
                Default::default(),
            )
            .is_ok()
            {
                let x = (wa.right - width - 20).max(wa.left + 12);
                let y = (wa.bottom - height - 20).max(wa.top + 12);
                return (x, y);
            }
        }
        (1100, 560)
    }

    pub fn open_folder_in_explorer(path: &Path) {
        let _ = std::fs::create_dir_all(path);
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }

    fn copy_wide_fixed<const N: usize>(dest: &mut [u16; N], text: &str) {
        dest.fill(0);
        for (i, unit) in text.encode_utf16().take(N.saturating_sub(1)).enumerate() {
            dest[i] = unit;
        }
    }

    /// Creates a crisp 32x32 ARGB battery tray icon via GDI DIB section.
    unsafe fn create_battery_tray_icon() -> Option<HICON> {
        const SIZE: i32 = 32;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let hdc = unsafe { GetDC(None) };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(hbm_color) =
            (unsafe { CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) })
        else {
            unsafe { ReleaseDC(None, hdc) };
            return None;
        };
        let hbm_mask = unsafe { CreateCompatibleBitmap(hdc, SIZE, SIZE) };
        unsafe { ReleaseDC(None, hdc) };

        if bits.is_null() {
            unsafe {
                let _ = DeleteObject(hbm_color.into());
                let _ = DeleteObject(hbm_mask.into());
            }
            return None;
        }

        let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (SIZE * SIZE) as usize) };
        pixels.fill(0x0000_0000);

        let set_px = |px_buf: &mut [u32], x: i32, y: i32, argb: u32| {
            if (0..SIZE).contains(&x) && (0..SIZE).contains(&y) {
                px_buf[(y * SIZE + x) as usize] = argb;
            }
        };

        for y in 2..30 {
            for x in 2..30 {
                let dx = if x < 7 { 7 - x } else if x > 24 { x - 24 } else { 0 };
                let dy = if y < 7 { 7 - y } else if y > 24 { y - 24 } else { 0 };
                if dx * dx + dy * dy <= 25 {
                    set_px(pixels, x, y, 0xFF14_1E30);
                }
            }
        }
        for x in 7..=24 {
            set_px(pixels, x, 2, 0xFF38_BDF8);
            set_px(pixels, x, 29, 0xFF38_BDF8);
        }
        for y in 7..=24 {
            set_px(pixels, 2, y, 0xFF38_BDF8);
            set_px(pixels, 29, y, 0xFF38_BDF8);
        }

        for y in 6..8 {
            for x in 13..19 {
                set_px(pixels, x, y, 0xFFE2_E8F0);
            }
        }
        for x in 10..22 {
            set_px(pixels, x, 8, 0xFFE2_E8F0);
            set_px(pixels, x, 9, 0xFFE2_E8F0);
            set_px(pixels, x, 24, 0xFFE2_E8F0);
            set_px(pixels, x, 25, 0xFFE2_E8F0);
        }
        for y in 8..26 {
            set_px(pixels, 10, y, 0xFFE2_E8F0);
            set_px(pixels, 11, y, 0xFFE2_E8F0);
            set_px(pixels, 20, y, 0xFFE2_E8F0);
            set_px(pixels, 21, y, 0xFFE2_E8F0);
        }
        for y in 12..23 {
            if y == 15 || y == 19 {
                continue;
            }
            for x in 13..19 {
                set_px(pixels, x, y, 0xFF22_C55E);
            }
        }

        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let icon = unsafe { CreateIconIndirect(&icon_info) }.ok();
        unsafe {
            let _ = DeleteObject(hbm_color.into());
            let _ = DeleteObject(hbm_mask.into());
        }
        icon
    }

    struct SharedTrayData {
        tooltip: String,
        state: TraySyncState,
        balloon_title: String,
        balloon_body: String,
        tx: smol::channel::Sender<TrayAction>,
    }

    thread_local! {
        static TRAY_CTX: std::cell::RefCell<Option<(Arc<Mutex<SharedTrayData>>, HICON)>> =
            const { std::cell::RefCell::new(None) };
    }

    fn emit_tray_action(action: TrayAction) {
        TRAY_CTX.with(|cell| {
            if let Some((shared, _)) = cell.borrow().as_ref()
                && let Ok(guard) = shared.lock()
            {
                let _ = guard.tx.send_blocking(action);
            }
        });
    }

    unsafe extern "system" fn tray_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TRAY_CALLBACK => {
                let mouse_msg = lparam.0 as u32;
                if mouse_msg == WM_LBUTTONUP {
                    emit_tray_action(TrayAction::ToggleVisible);
                } else if mouse_msg == WM_RBUTTONUP {
                    let sync_state = TRAY_CTX.with(|cell| {
                        cell.borrow()
                            .as_ref()
                            .and_then(|(s, _)| s.lock().ok().map(|g| g.state.clone()))
                    });
                    if let Some(st) = sync_state {
                        unsafe { show_context_menu(hwnd, &st) };
                    }
                }
                LRESULT(0)
            }
            WM_TRAY_WAKE => {
                emit_tray_action(TrayAction::ShowAndFocus);
                LRESULT(0)
            }
            WM_TRAY_SYNC => {
                TRAY_CTX.with(|cell| {
                    if let Some((shared, hicon)) = cell.borrow().as_ref()
                        && let Ok(guard) = shared.lock()
                    {
                        let mut nid = NOTIFYICONDATAW {
                            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                            hWnd: hwnd,
                            uID: 1,
                            uFlags: NIF_TIP | NIF_ICON,
                            hIcon: *hicon,
                            ..Default::default()
                        };
                        copy_wide_fixed(&mut nid.szTip, &guard.tooltip);
                        unsafe {
                            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_TRAY_BALLOON => {
                TRAY_CTX.with(|cell| {
                    if let Some((shared, _)) = cell.borrow().as_ref()
                        && let Ok(guard) = shared.lock()
                    {
                        let mut nid = NOTIFYICONDATAW {
                            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                            hWnd: hwnd,
                            uID: 1,
                            uFlags: NIF_INFO,
                            dwInfoFlags: NIIF_WARNING,
                            ..Default::default()
                        };
                        copy_wide_fixed(&mut nid.szInfoTitle, &guard.balloon_title);
                        copy_wide_fixed(&mut nid.szInfo, &guard.balloon_body);
                        unsafe {
                            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_TRAY_QUIT => {
                let nid = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: hwnd,
                    uID: 1,
                    ..Default::default()
                };
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let nid = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: hwnd,
                    uID: 1,
                    ..Default::default()
                };
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    unsafe fn show_context_menu(hwnd: HWND, state: &TraySyncState) {
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };

        let vis_label = if state.visible {
            w!("隐藏小组件")
        } else {
            w!("显示小组件")
        };
        let pin_flags = if state.always_on_top {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let compact_flags = if state.compact_mode {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };

        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, CMD_TOGGLE_VISIBLE, vis_label);
            let _ = AppendMenuW(menu, MF_STRING, CMD_REFRESH, w!("立即刷新电量"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, pin_flags, CMD_TOGGLE_PIN, w!("始终置顶"));
            let _ = AppendMenuW(menu, compact_flags, CMD_TOGGLE_COMPACT, w!("精简悬浮条"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_SCANNER, w!("设备扫描 / 插件管理"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("退出程序"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);

            let action = match cmd.0 as usize {
                CMD_TOGGLE_VISIBLE => Some(TrayAction::ToggleVisible),
                CMD_REFRESH => Some(TrayAction::RefreshNow),
                CMD_TOGGLE_PIN => Some(TrayAction::TogglePin),
                CMD_TOGGLE_COMPACT => Some(TrayAction::ToggleCompact),
                CMD_OPEN_SCANNER => Some(TrayAction::OpenScanner),
                CMD_QUIT => Some(TrayAction::Quit),
                _ => None,
            };
            if let Some(act) = action {
                emit_tray_action(act);
            }
        }
    }

    pub struct TrayController {
        hwnd_raw: isize,
        shared: Arc<Mutex<SharedTrayData>>,
        shutdown_sent: AtomicBool,
        join_handle: Mutex<Option<JoinHandle<()>>>,
    }

    impl Drop for TrayController {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    impl TrayController {
        pub fn start(
            initial_state: TraySyncState,
        ) -> (Self, smol::channel::Receiver<TrayAction>) {
            let (tx, rx) = smol::channel::unbounded::<TrayAction>();
            let shared = Arc::new(Mutex::new(SharedTrayData {
                tooltip: "设备电量小组件".to_string(),
                state: initial_state,
                balloon_title: String::new(),
                balloon_body: String::new(),
                tx,
            }));

            let (hwnd_tx, hwnd_rx) = std::sync::mpsc::sync_channel::<isize>(1);
            let shared_bg = Arc::clone(&shared);

            let handle = thread::Builder::new()
                .name("tray-loop".to_string())
                .spawn(move || unsafe {
                    let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(tray_wnd_proc),
                        hInstance: hinst,
                        lpszClassName: TRAY_CLASS_NAME,
                        ..Default::default()
                    };
                    RegisterClassW(&wc);

                    let hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        TRAY_CLASS_NAME,
                        w!("BatteryDisplayTray"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(hinst),
                        None,
                    )
                    .unwrap_or_default();

                    let hicon = create_battery_tray_icon().unwrap_or_default();
                    TRAY_CTX.with(|cell| {
                        *cell.borrow_mut() = Some((shared_bg, hicon));
                    });

                    let mut nid = NOTIFYICONDATAW {
                        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: hwnd,
                        uID: 1,
                        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                        uCallbackMessage: WM_TRAY_CALLBACK,
                        hIcon: hicon,
                        ..Default::default()
                    };
                    copy_wide_fixed(&mut nid.szTip, "设备电量小组件 - 点击显示/隐藏");
                    let _ = Shell_NotifyIconW(NIM_ADD, &nid);

                    let _ = hwnd_tx.send(hwnd.0 as isize);

                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }

                    TRAY_CTX.with(|cell| {
                        *cell.borrow_mut() = None;
                    });
                    if !hicon.is_invalid() {
                        let _ = DestroyIcon(hicon);
                    }
                })
                .expect("failed to spawn tray thread");

            let hwnd_raw = hwnd_rx.recv().unwrap_or(0);
            (
                Self {
                    hwnd_raw,
                    shared,
                    shutdown_sent: AtomicBool::new(false),
                    join_handle: Mutex::new(Some(handle)),
                },
                rx,
            )
        }

        pub fn update_tooltip_and_state(&self, tooltip: String, state: TraySyncState) {
            if let Ok(mut guard) = self.shared.lock() {
                guard.tooltip = tooltip;
                guard.state = state;
            }
            if self.hwnd_raw != 0 {
                unsafe {
                    let _ = PostMessageW(
                        Some(to_hwnd(self.hwnd_raw)),
                        WM_TRAY_SYNC,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }

        pub fn show_balloon(&self, title: String, body: String) {
            if let Ok(mut guard) = self.shared.lock() {
                guard.balloon_title = title;
                guard.balloon_body = body;
            }
            if self.hwnd_raw != 0 {
                unsafe {
                    let _ = PostMessageW(
                        Some(to_hwnd(self.hwnd_raw)),
                        WM_TRAY_BALLOON,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }

        pub fn shutdown(&self) {
            if self.shutdown_sent.swap(true, Ordering::SeqCst) {
                return;
            }
            if self.hwnd_raw != 0 {
                unsafe {
                    let _ = PostMessageW(
                        Some(to_hwnd(self.hwnd_raw)),
                        WM_TRAY_QUIT,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            if let Ok(mut slot) = self.join_handle.lock()
                && let Some(h) = slot.take()
            {
                let _ = h.join();
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::{
    SingleInstanceGuard, TrayController, apply_window_style, attach_parent_console,
    default_widget_position, get_monitor_work_area, get_monitor_work_area_for_point,
    get_scale_factor_for_point, get_window_frame_insets, get_window_rect, open_folder_in_explorer,
    set_window_bounds, set_window_visible, set_window_visible_and_focus,
    wake_existing_instance, write_cli_output,
};

#[cfg(not(target_os = "windows"))]
pub fn attach_parent_console() {}
#[cfg(not(target_os = "windows"))]
pub fn write_cli_output(text: &str) {
    print!("{text}");
}
#[cfg(not(target_os = "windows"))]
pub fn apply_window_style(_: isize, _: bool) {}
#[cfg(not(target_os = "windows"))]
pub fn get_window_rect(_: isize) -> Option<RectI32> {
    None
}
#[cfg(not(target_os = "windows"))]
pub fn get_window_frame_insets(_: isize) -> (i32, i32) {
    (0, 0)
}
#[cfg(not(target_os = "windows"))]
pub fn get_monitor_work_area(_: isize) -> Option<RectI32> {
    None
}
#[cfg(not(target_os = "windows"))]
pub fn get_monitor_work_area_for_point(_: i32, _: i32) -> RectI32 {
    RectI32::from_xywh(0, 0, 1920, 1040)
}
#[cfg(not(target_os = "windows"))]
pub fn get_scale_factor_for_point(_: i32, _: i32) -> f32 {
    1.0
}
#[cfg(not(target_os = "windows"))]
pub fn set_window_bounds(_: isize, _: i32, _: i32, _: i32, _: i32) {}
#[cfg(not(target_os = "windows"))]
pub fn set_window_visible(_: isize, _: bool) {}
#[cfg(not(target_os = "windows"))]
pub fn set_window_visible_and_focus(_: isize, _: bool, _: bool) {}
#[cfg(not(target_os = "windows"))]
pub fn wake_existing_instance() -> bool {
    false
}
#[cfg(not(target_os = "windows"))]
pub fn default_widget_position(_: i32, _: i32) -> (i32, i32) {
    (100, 100)
}
#[cfg(not(target_os = "windows"))]
pub fn open_folder_in_explorer(_: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn wa_1080() -> RectI32 {
        RectI32::from_xywh(0, 0, 1920, 1040)
    }

    #[test]
    fn left_top_dock_expands_right_and_down() {
        let cur = RectI32::from_xywh(16, 20, 260, 44);
        let (ax, ay) = infer_anchors(cur, wa_1080());
        assert_eq!((ax, ay), (AnchorX::Left, AnchorY::Top));

        let next = compute_anchored_rect(cur, wa_1080(), 420, 240, ax, ay);
        assert_eq!(next.left, 16);
        assert_eq!(next.top, 20);
        assert_eq!(next.width(), 420);
        assert_eq!(next.height(), 240);
    }

    #[test]
    fn right_bottom_dock_expands_left_and_up() {
        // Widget sits near bottom-right corner
        let cur = RectI32::from_xywh(1640, 860, 260, 160);
        let (ax, ay) = infer_anchors(cur, wa_1080());
        assert_eq!((ax, ay), (AnchorX::Right, AnchorY::Bottom));

        // Expand width and height -> right edge (1900) and bottom edge (1020) stay pinned!
        let next = compute_anchored_rect(cur, wa_1080(), 400, 320, ax, ay);
        assert_eq!(next.right, 1900);
        assert_eq!(next.bottom, 1020);
        assert_eq!(next.left, 1500);
        assert_eq!(next.top, 700);

        // Shrink back -> right and bottom still pinned
        let shrunk = compute_anchored_rect(next, wa_1080(), 220, 48, ax, ay);
        assert_eq!(shrunk.right, 1900);
        assert_eq!(shrunk.bottom, 1020);
        assert_eq!(shrunk.left, 1680);
    }

    #[test]
    fn overflow_clamps_inside_work_area() {
        let wa = RectI32::from_xywh(100, 50, 1200, 800);
        // Near right boundary but Left-anchored (e.g. slightly past center) -> expanding wider must shift left
        let cur = RectI32::from_xywh(950, 700, 200, 100);
        let next = compute_anchored_rect(cur, wa, 500, 300, AnchorX::Left, AnchorY::Top);
        assert_eq!(next.right, wa.right);
        assert_eq!(next.left, wa.right - 500);
        assert_eq!(next.bottom, wa.bottom);
        assert_eq!(next.top, wa.bottom - 300);

        // Near left boundary -> even if Right-anchored, expanding wider must shift right so left >= wa.left
        let cur_l = RectI32::from_xywh(120, 80, 200, 100);
        let next_l = compute_anchored_rect(cur_l, wa, 450, 260, AnchorX::Right, AnchorY::Bottom);
        assert_eq!(next_l.left, wa.left);
        assert_eq!(next_l.right, wa.left + 450);
        assert_eq!(next_l.top, wa.top);
    }
}
