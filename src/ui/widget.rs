use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Theme, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{AppConfig, ThemePreference, sync_autostart_registry};
use crate::manager::{
    PollCommand, PollStateTracker, filter_visible_devices, format_tray_tooltip,
};
use crate::model::{ConnectionType, DeviceKind, DeviceReading, PollReport, PowerState};
use crate::platform_win::{
    AnchorX, AnchorY, RectI32, TrayAction, TrayController, TraySyncState, apply_window_style,
    compute_anchored_rect, get_monitor_work_area, get_monitor_work_area_for_point,
    get_window_frame_insets, get_window_rect, hwnd_from_gpui_window, infer_anchors,
    open_folder_in_explorer, set_window_bounds, set_window_visible,
    set_window_visible_and_focus,
};
use crate::plugins::runner::{LoadedPlugin, PluginLoadError, load_all_plugins};
use crate::plugins::scanner::{
    ProbeRequest, ProbeResult, ScanSnapshot, ScannedBtDevice, ScannedHidCollection,
    SupportStatus, build_bt_plugin_from_scan, build_hid_plugin_from_scan, delete_user_plugin,
    extract_candidate_battery_offsets, run_device_scan, run_hid_probe, save_plugin_to_user_dir,
};
use crate::plugins::schema::HidProtocolMode;
use crate::ui::Palette;
use crate::ui::device_card::{render_compact_chip, render_device_card};
use crate::ui::scanner::{ScannerTab, render_scanner_panel};
use crate::ui::settings::render_settings_panel;

pub const CARD_WIDTH: f32 = 322.0;
pub const SCANNER_WIDTH: f32 = 440.0;

#[derive(Debug, Clone, Default)]
pub struct DemoOptions {
    pub enabled: bool,
    pub light_theme: bool,
    pub compact: bool,
    pub compact_max_per_row: usize,
    pub open_settings: bool,
    pub open_scanner: bool,
}

pub struct BatteryWidget {
    demo_mode: bool,
    config: AppConfig,
    report: PollReport,
    tracker: PollStateTracker,
    polling: bool,
    visible: bool,
    show_settings: bool,
    show_diagnostics: bool,
    pub show_scanner: bool,
    pub scanner_tab: ScannerTab,
    pub scanner_vendor_only: bool,
    pub scanning_devices: bool,
    pub probing_hid: bool,
    pub scan_snapshot: ScanSnapshot,
    pub selected_hid_col: Option<ScannedHidCollection>,
    pub probe_mode: HidProtocolMode,
    pub probe_xor_ff: bool,
    pub probe_result: Option<ProbeResult>,
    pub scanner_kind: DeviceKind,
    pub scanner_status: Option<(bool, String)>,
    pub loaded_plugins: Vec<LoadedPlugin>,
    pub plugin_errors: Vec<PluginLoadError>,
    pub input_plugin_name: Entity<InputState>,
    pub input_probe_hex: Entity<InputState>,
    pub input_level_offset: Entity<InputState>,
    pub input_charging_offset: Entity<InputState>,
    window_handle: AnyWindowHandle,
    hwnd: Option<isize>,
    anchor_x: AnchorX,
    anchor_y: AnchorY,
    last_applied_rect: Option<RectI32>,
    last_target_size: (f32, f32),
    resize_gen: Arc<AtomicU64>,
    poll_cmd_tx: mpsc::Sender<PollCommand>,
    tray: Option<TrayController>,
}

impl BatteryWidget {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mut config: AppConfig,
        initial_phys_pos: (i32, i32),
        poll_cmd_tx: mpsc::Sender<PollCommand>,
        report_rx: smol::channel::Receiver<PollReport>,
        tray: Option<TrayController>,
        tray_rx: Option<smol::channel::Receiver<TrayAction>>,
        demo: DemoOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hwnd = hwnd_from_gpui_window(window);
        if demo.enabled {
            config.theme_mode = if demo.light_theme {
                ThemePreference::Light
            } else {
                ThemePreference::Dark
            };
            config.compact_mode = demo.compact;
            if demo.compact_max_per_row > 0 {
                config.compact_max_per_row = demo.compact_max_per_row.clamp(1, 6);
            }
        }
        let target_mode = match config.theme_mode {
            ThemePreference::Light => ThemeMode::Light,
            ThemePreference::Dark | ThemePreference::System => ThemeMode::Dark,
        };
        Theme::change(target_mode, Some(window), cx);

        let input_plugin_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("设备名称 (例如 ATK F1)"));
        let input_probe_hex = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("11 06")
                .placeholder("测试指令 hex，如 11 06 或 55 65 01")
        });
        let input_level_offset = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("11")
                .placeholder("偏移")
        });
        let input_charging_offset =
            cx.new(|cx| InputState::new(window, cx).placeholder("可选"));

        let (loaded_plugins, plugin_errors) = load_all_plugins();
        let window_handle = window.window_handle();

        // Background task receiving polled battery reports
        cx.spawn(async move |this, cx| {
            while let Ok(report) = report_rx.recv().await {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        view.on_new_report(report, window, cx);
                    });
                });
            }
        })
        .detach();

        // Background task receiving Win32 system tray events & instance wake-up
        if let Some(trx) = tray_rx {
            cx.spawn(async move |this, cx| {
                while let Ok(action) = trx.recv().await {
                    let _ = cx.update_window(window_handle, |_, window, cx| {
                        let _ = this.update(cx, |view, cx| {
                            view.on_tray_action(action, window, cx);
                        });
                    });
                }
            })
            .detach();
        }

        // Periodic relative-time refresh + drag position persistence every 12s
        cx.spawn(async move |this, cx| {
            loop {
                smol::Timer::after(Duration::from_secs(12)).await;
                let res = cx.update_window(window_handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        view.sync_window_position(window);
                        cx.notify();
                    });
                });
                if res.is_err() {
                    break;
                }
            }
        })
        .detach();

        let wa = get_monitor_work_area_for_point(initial_phys_pos.0, initial_phys_pos.1);
        let init_rect = RectI32::from_xywh(initial_phys_pos.0, initial_phys_pos.1, 320, 120);
        let (anchor_x, anchor_y) = if config.window_x.is_some() && config.window_y.is_some() {
            infer_anchors(init_rect, wa)
        } else {
            (AnchorX::Right, AnchorY::Bottom)
        };

        let mut report = PollReport::default();
        let mut polling = true;
        let mut show_settings = false;
        let mut show_scanner = false;
        let mut scan_snapshot = ScanSnapshot::default();
        let mut selected_hid_col = None;
        let mut probe_xor_ff = false;
        let mut probe_result = None;

        if demo.enabled {
            polling = false;
            show_settings = demo.open_settings;
            show_scanner = demo.open_scanner;
            let now = Local::now();
            report = PollReport {
                devices: vec![
                    DeviceReading {
                        id: "mchose:mouse_5253".into(),
                        name: "MCHOSE A7 Ultra".into(),
                        brand: Some("MCHOSE".into()),
                        kind: DeviceKind::Mouse,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: Some(74),
                        power_state: PowerState::Discharging,
                        provider: "mchose_mouse".into(),
                        vid_pid: Some((0x5253, 0x1020)),
                        mac_address: None,
                        last_seen: now,
                        approximate: false,
                        detail: Some("型号 0x0011".into()),
                    },
                    DeviceReading {
                        id: "mchose:v9_pro".into(),
                        name: "MCHOSE V9 PRO".into(),
                        brand: Some("MCHOSE".into()),
                        kind: DeviceKind::Headset,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: Some(86),
                        power_state: PowerState::Discharging,
                        provider: "mchose_headset".into(),
                        vid_pid: Some((0x291D, 0x385D)),
                        mac_address: None,
                        last_seen: now,
                        approximate: false,
                        detail: None,
                    },
                    DeviceReading {
                        id: "gamesir:2106".into(),
                        name: "GameSir Nova 2 Lite".into(),
                        brand: Some("GameSir".into()),
                        kind: DeviceKind::Gamepad,
                        connection: ConnectionType::Wired,
                        battery_percent: Some(62),
                        power_state: PowerState::Charging,
                        provider: "gamesir_hid".into(),
                        vid_pid: Some((0x3537, 0x2106)),
                        mac_address: None,
                        last_seen: now,
                        approximate: false,
                        detail: None,
                    },
                    DeviceReading {
                        id: "bt:FCE806A6742F".into(),
                        name: "EDIFIER W820NB".into(),
                        brand: Some("EDIFIER".into()),
                        kind: DeviceKind::Headset,
                        connection: ConnectionType::Bluetooth,
                        battery_percent: Some(18),
                        power_state: PowerState::Discharging,
                        provider: "bluetooth".into(),
                        vid_pid: None,
                        mac_address: Some("FCE806A6742F".into()),
                        last_seen: now,
                        approximate: false,
                        detail: None,
                    },
                ],
                diagnostics: vec![
                    "[system] enumerated 25 HID interfaces, 0 active plugins".into(),
                    "[mchose-mouse] 5253:1020 rid=0x11 -> 74% charging=false".into(),
                    "[mchose-headset] 291D:385D UP=FF90 -> 86%".into(),
                    "[gamesir] 3537:2106 UP=FF7A -> charging (62%)".into(),
                    "[bluetooth] mac=FCE806A6742F 'EDIFIER W820NB' connected bat=18%".into(),
                ],
                polled_at: now,
            };
            if demo.open_scanner {
                scan_snapshot = run_device_scan(&loaded_plugins);
                selected_hid_col = scan_snapshot
                    .hid_groups
                    .iter()
                    .flat_map(|g| g.collections.iter())
                    .find(|c| c.vid == 0x5253 && c.usage_page == 0xFF01)
                    .or_else(|| {
                        scan_snapshot
                            .hid_groups
                            .iter()
                            .flat_map(|g| g.collections.iter())
                            .find(|c| c.is_vendor_channel)
                    })
                    .cloned()
                    .or_else(|| {
                        Some(ScannedHidCollection {
                            path: r"\\?\hid#vid_5253&pid_1020&mi_02".into(),
                            vid: 0x5253,
                            pid: 0x1020,
                            usage_page: 0xFF01,
                            usage: 0x0001,
                            input_len: 65,
                            output_len: 65,
                            feature_len: 65,
                            interface_number: Some(2),
                            manufacturer: "RealTek".into(),
                            product: "MCHOSE A7 Ultra".into(),
                            is_vendor_channel: true,
                            support: SupportStatus::BuiltIn("MCHOSE"),
                        })
                    });
                if let Some(ref sel) = selected_hid_col {
                    let pname = sel.product.clone();
                    input_plugin_name.update(cx, |st, cx| {
                        st.set_value(pname, window, cx);
                    });
                    input_charging_offset.update(cx, |st, cx| {
                        st.set_value("12", window, cx);
                    });
                }
                probe_xor_ff = true;
                let sample_decoded = vec![
                    0x11u8, 0x06, 0x53, 0x52, 0x11, 0x00, 0x05, 0x00, 0x07, 0x04, 0x09, 0x4A, 0x00,
                ];
                probe_result = Some(ProbeResult {
                    ok: true,
                    raw_hex: "11 F9 AC AD EE FF FA FF F8 FB F6 B5 FF".into(),
                    decoded_hex: "11 06 53 52 11 00 05 00 07 04 09 4A 00".into(),
                    candidate_offsets: extract_candidate_battery_offsets(&sample_decoded),
                    decoded_bytes: sample_decoded,
                    message: "收到 21 字节响应，点击下方高亮候选字节即可填入电量偏移".into(),
                });
            }
        }

        let mut widget = Self {
            demo_mode: demo.enabled,
            config,
            report,
            tracker: PollStateTracker::default(),
            polling,
            visible: true,
            show_settings,
            show_diagnostics: false,
            show_scanner,
            scanner_tab: ScannerTab::Hid,
            scanner_vendor_only: true,
            scanning_devices: false,
            probing_hid: false,
            scan_snapshot,
            selected_hid_col,
            probe_mode: HidProtocolMode::FeatureReport,
            probe_xor_ff,
            probe_result,
            scanner_kind: DeviceKind::Mouse,
            scanner_status: None,
            loaded_plugins,
            plugin_errors,
            input_plugin_name,
            input_probe_hex,
            input_level_offset,
            input_charging_offset,
            window_handle,
            hwnd,
            anchor_x,
            anchor_y,
            last_applied_rect: None,
            last_target_size: (0.0, 0.0),
            resize_gen: Arc::new(AtomicU64::new(0)),
            poll_cmd_tx,
            tray,
        };

        // Defer Win32 window style & initial bounds alignment AFTER open_window returns
        // so Windows WM_MOVE/WM_DPICHANGED never re-enters while GPUI AppCell is borrowed.
        if let Some(h) = hwnd {
            let on_top = widget.config.always_on_top;
            cx.foreground_executor()
                .spawn(async move {
                    apply_window_style(h, on_top);
                })
                .detach();
            widget.schedule_window_resize(window, cx, true);
        }

        widget
    }

    fn save_config_if_persistent(&self) {
        if !self.demo_mode {
            let _ = self.config.save();
        }
    }

    fn visible_devices(&self) -> Vec<&DeviceReading> {
        filter_visible_devices(
            &self.report.devices,
            self.config.show_sleeping,
            self.config.show_phones,
            &self.config.hidden_device_ids,
        )
    }

    fn effective_compact_per_row(&self, max_work_w_logical: f32) -> usize {
        let avail_for_chips = (max_work_w_logical - 120.0).max(80.0);
        let max_fit = (((avail_for_chips + 6.0) / 86.0).floor() as usize).max(1);
        self.config.compact_max_per_row.clamp(1, max_fit.min(6))
    }

    pub fn compute_target_size(&self, max_work_w: f32, max_work_h: f32) -> (f32, f32) {
        let count = self.visible_devices().len();
        if self.config.compact_mode {
            if count == 0 {
                return (196.0, 42.0);
            }
            let per_row = self.effective_compact_per_row(max_work_w);
            let cols = count.min(per_row);
            let rows = count.div_ceil(per_row).min(4);
            let chips_w = (cols as f32) * 80.0 + (cols.saturating_sub(1) as f32) * 6.0;
            let width = (32.0 + chips_w + 8.0 + 54.0).min(max_work_w.max(196.0));
            let height = ((rows as f32) * 28.0 + (rows.saturating_sub(1) as f32) * 6.0 + 14.0)
                .clamp(42.0, max_work_h.max(42.0));
            return (width, height);
        }

        let width = if self.show_scanner {
            SCANNER_WIDTH
        } else {
            CARD_WIDTH
        }
        .min(max_work_w.max(280.0));

        let header_h = 42.0;
        let list_h = if count == 0 {
            82.0
        } else {
            let max_cards = if self.show_scanner { 2 } else { 6 };
            let visible_cards = count.min(max_cards) as f32;
            visible_cards * 64.0 + (visible_cards - 1.0).max(0.0) * 8.0 + 24.0
        };

        let settings_h = if self.show_settings {
            if self.show_diagnostics { 386.0 } else { 262.0 }
        } else {
            0.0
        };

        let scanner_h = if self.show_scanner {
            let status_extra = if self.scanner_status.is_some() { 28.0 } else { 0.0 };
            let tab_h = match self.scanner_tab {
                ScannerTab::Hid => {
                    if self.selected_hid_col.is_some() {
                        if self.probe_result.as_ref().is_some_and(|r| r.ok) {
                            438.0
                        } else if self.probe_result.is_some() {
                            374.0
                        } else {
                            346.0
                        }
                    } else {
                        222.0
                    }
                }
                ScannerTab::Bluetooth | ScannerTab::Plugins => 304.0,
            };
            tab_h + status_extra
        } else {
            0.0
        };

        let total_h = (header_h + list_h + settings_h + scanner_h).clamp(118.0, max_work_h.max(180.0));
        (width, total_h)
    }

    fn refresh_anchors_if_dragged(&mut self, cur: RectI32, wa: RectI32) {
        let moved_externally = match self.last_applied_rect {
            Some(prev) => (cur.left - prev.left).abs() > 4 || (cur.top - prev.top).abs() > 4,
            None => self.config.window_x.is_some() && self.config.window_y.is_some(),
        };
        if moved_externally {
            let (ax, ay) = infer_anchors(cur, wa);
            self.anchor_x = ax;
            self.anchor_y = ay;
        }
    }

    fn schedule_window_resize(&mut self, window: &Window, cx: &mut Context<Self>, force: bool) {
        let Some(h) = self.hwnd else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let cur = get_window_rect(h).unwrap_or_else(|| {
            let x = self.config.window_x.unwrap_or(100);
            let y = self.config.window_y.unwrap_or(100);
            RectI32::from_xywh(x, y, (CARD_WIDTH * scale) as i32, (140.0 * scale) as i32)
        });
        let wa = get_monitor_work_area(h)
            .unwrap_or_else(|| get_monitor_work_area_for_point(cur.left, cur.top));

        self.refresh_anchors_if_dragged(cur, wa);

        let max_work_w = (wa.width() as f32 / scale - 16.0).max(240.0);
        let max_work_h = (wa.height() as f32 / scale - 16.0).max(160.0);
        let target_logical = self.compute_target_size(max_work_w, max_work_h);

        let size_changed = (target_logical.0 - self.last_target_size.0).abs() > 0.5
            || (target_logical.1 - self.last_target_size.1).abs() > 0.5;
        if !force && !size_changed {
            return;
        }
        self.last_target_size = target_logical;

        let (inset_w, inset_h) = get_window_frame_insets(h);
        let phys_w = (target_logical.0 * scale).round() as i32 + inset_w;
        let phys_h = (target_logical.1 * scale).round() as i32 + inset_h;

        let next =
            compute_anchored_rect(cur, wa, phys_w, phys_h, self.anchor_x, self.anchor_y);
        self.last_applied_rect = Some(next);
        self.config.window_x = Some(next.left);
        self.config.window_y = Some(next.top);

        let seq = self.resize_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let seq_ref = Arc::clone(&self.resize_gen);
        cx.foreground_executor()
            .spawn(async move {
                if seq_ref.load(Ordering::SeqCst) == seq {
                    set_window_bounds(h, next.left, next.top, next.width(), next.height());
                }
            })
            .detach();
    }

    fn sync_window_position(&mut self, _window: &Window) {
        if self.demo_mode {
            return;
        }
        let Some(h) = self.hwnd else {
            return;
        };
        let Some(cur) = get_window_rect(h) else {
            return;
        };
        let wa = get_monitor_work_area(h)
            .unwrap_or_else(|| get_monitor_work_area_for_point(cur.left, cur.top));
        self.refresh_anchors_if_dragged(cur, wa);
        self.last_applied_rect = Some(cur);
        if self.config.window_x != Some(cur.left) || self.config.window_y != Some(cur.top) {
            self.config.window_x = Some(cur.left);
            self.config.window_y = Some(cur.top);
            self.save_config_if_persistent();
        }
    }

    fn sync_tray(&self) {
        if let Some(tray) = &self.tray {
            let tip = format_tray_tooltip(&self.report.devices);
            tray.update_tooltip_and_state(
                tip,
                TraySyncState {
                    visible: self.visible,
                    always_on_top: self.config.always_on_top,
                    compact_mode: self.config.compact_mode,
                },
            );
        }
    }

    fn on_new_report(&mut self, report: PollReport, window: &mut Window, cx: &mut Context<Self>) {
        self.polling = false;
        let alerts = self.tracker.collect_low_battery_alerts(
            &report.devices,
            self.config.low_battery_threshold,
            self.config.low_battery_notify,
        );
        self.report = report;

        if !alerts.is_empty()
            && let Some(tray) = &self.tray
        {
            for d in alerts {
                let name = self.config.resolve_name(&d.id, &d.name);
                let pct = d.battery_percent.unwrap_or(0);
                tray.show_balloon(
                    "设备低电量提醒".to_string(),
                    format!("{name} 电量仅剩 {pct}%，请及时充电"),
                );
            }
        }

        self.schedule_window_resize(window, cx, false);
        self.sync_tray();
        cx.notify();
    }

    fn show_and_focus_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        if let Some(h) = self.hwnd {
            let on_top = self.config.always_on_top;
            cx.foreground_executor()
                .spawn(async move {
                    set_window_visible_and_focus(h, true, on_top);
                })
                .detach();
        }
        self.schedule_window_resize(window, cx, true);
        self.sync_tray();
        cx.notify();
    }

    fn on_tray_action(&mut self, action: TrayAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            TrayAction::ToggleVisible => {
                self.visible = !self.visible;
                if let Some(h) = self.hwnd {
                    let vis = self.visible;
                    let on_top = self.config.always_on_top;
                    cx.foreground_executor()
                        .spawn(async move {
                            set_window_visible(h, vis);
                            if vis {
                                apply_window_style(h, on_top);
                            }
                        })
                        .detach();
                }
                self.sync_tray();
                cx.notify();
            }
            TrayAction::ShowAndFocus => {
                self.show_and_focus_window(window, cx);
            }
            TrayAction::OpenScanner => {
                self.config.compact_mode = false;
                self.show_settings = false;
                self.show_scanner = true;
                self.reload_all_plugins_now(cx);
                self.trigger_device_scan(cx);
                self.show_and_focus_window(window, cx);
            }
            TrayAction::RefreshNow => {
                self.trigger_refresh(cx);
            }
            TrayAction::TogglePin => {
                self.toggle_always_on_top(window, cx);
            }
            TrayAction::ToggleCompact => {
                self.toggle_compact_mode(window, cx);
            }
            TrayAction::Quit => {
                self.sync_window_position(window);
                let _ = self.poll_cmd_tx.send(PollCommand::Shutdown);
                if let Some(tray) = &self.tray {
                    tray.shutdown();
                }
                cx.quit();
            }
        }
    }

    pub fn trigger_refresh(&mut self, cx: &mut Context<Self>) {
        self.polling = true;
        let _ = self.poll_cmd_tx.send(PollCommand::PollNow);
        cx.notify();
    }

    pub fn toggle_compact_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.compact_mode = !self.config.compact_mode;
        if self.config.compact_mode {
            self.show_settings = false;
            self.show_scanner = false;
        }
        self.schedule_window_resize(window, cx, true);
        self.save_config_if_persistent();
        self.sync_tray();
        cx.notify();
    }

    pub fn toggle_always_on_top(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.config.always_on_top = !self.config.always_on_top;
        if let Some(h) = self.hwnd {
            let on_top = self.config.always_on_top;
            cx.foreground_executor()
                .spawn(async move {
                    apply_window_style(h, on_top);
                })
                .detach();
        }
        self.save_config_if_persistent();
        self.sync_tray();
        cx.notify();
    }

    pub fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings = !self.show_settings;
        if self.show_settings {
            self.show_scanner = false;
        }
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn toggle_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_diagnostics = !self.show_diagnostics;
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn toggle_scanner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_scanner = !self.show_scanner;
        if self.show_scanner {
            self.config.compact_mode = false;
            self.show_settings = false;
            self.reload_all_plugins_now(cx);
            if self.scan_snapshot.hid_groups.is_empty() && !self.scanning_devices {
                self.trigger_device_scan(cx);
            }
        }
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn set_scanner_tab(&mut self, tab: ScannerTab, window: &mut Window, cx: &mut Context<Self>) {
        self.scanner_tab = tab;
        if tab == ScannerTab::Plugins {
            self.reload_all_plugins_now(cx);
        }
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn trigger_device_scan(&mut self, cx: &mut Context<Self>) {
        self.scanning_devices = true;
        let plugins = self.loaded_plugins.clone();
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let snap = cx
                .background_executor()
                .spawn(async move { run_device_scan(&plugins) })
                .await;
            let _ = cx.update_window(window_handle, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.scanning_devices = false;
                    view.scan_snapshot = snap;
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub fn toggle_vendor_only_filter(&mut self, cx: &mut Context<Self>) {
        self.scanner_vendor_only = !self.scanner_vendor_only;
        cx.notify();
    }

    pub fn select_hid_collection(
        &mut self,
        col: ScannedHidCollection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suggested_name = if col.product.trim().is_empty()
            || col.product.eq_ignore_ascii_case("USB Input Device")
        {
            format!("自定义设备 {:04X}:{:04X}", col.vid, col.pid)
        } else {
            col.product.trim().to_string()
        };
        self.input_plugin_name.update(cx, |state, cx| {
            state.set_value(suggested_name, window, cx);
        });
        if col.feature_len >= 10 && col.output_len == 0 {
            self.probe_mode = HidProtocolMode::FeatureReport;
        } else if col.output_len >= 16 && col.input_len >= 16 {
            self.probe_mode = HidProtocolMode::OutputInput;
        }
        self.selected_hid_col = Some(col);
        self.probe_result = None;
        self.scanner_status = None;
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn set_probe_mode(&mut self, mode: HidProtocolMode, cx: &mut Context<Self>) {
        self.probe_mode = mode;
        cx.notify();
    }

    pub fn toggle_probe_xor(&mut self, cx: &mut Context<Self>) {
        self.probe_xor_ff = !self.probe_xor_ff;
        cx.notify();
    }

    pub fn trigger_hid_probe(&mut self, cx: &mut Context<Self>) {
        let Some(col) = self.selected_hid_col.clone() else {
            return;
        };
        let request_hex = self.input_probe_hex.read(cx).value().to_string();
        let req = ProbeRequest {
            collection: col,
            mode: self.probe_mode,
            request_hex,
            xor_ff: self.probe_xor_ff,
            timeout_ms: 800,
        };
        self.probing_hid = true;
        self.scanner_status = None;
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { run_hid_probe(&req) })
                .await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.probing_hid = false;
                    view.probe_result = Some(res);
                    view.schedule_window_resize(window, cx, true);
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub fn apply_candidate_offset(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_level_offset.update(cx, |st, cx| {
            st.set_value(offset.to_string(), window, cx);
        });
        cx.notify();
    }

    pub fn set_scanner_kind(&mut self, kind: DeviceKind, cx: &mut Context<Self>) {
        self.scanner_kind = kind;
        cx.notify();
    }

    pub fn save_scanned_hid_plugin(&mut self, open_folder: bool, cx: &mut Context<Self>) {
        let Some(col) = self.selected_hid_col.clone() else {
            self.scanner_status = Some((true, "请先在上方列表中选择一个 HID 接口".into()));
            cx.notify();
            return;
        };
        let name = self.input_plugin_name.read(cx).value().to_string();
        let req_hex = self.input_probe_hex.read(cx).value().to_string();
        let lvl_str = self.input_level_offset.read(cx).value().to_string();
        let chg_str = self.input_charging_offset.read(cx).value().to_string();

        let Ok(level_offset) = lvl_str.trim().parse::<usize>() else {
            self.scanner_status = Some((true, "电量字节偏移必须为非负整数（如 11 或 19）".into()));
            cx.notify();
            return;
        };
        let charging_offset = if chg_str.trim().is_empty() {
            None
        } else {
            match chg_str.trim().parse::<usize>() {
                Ok(v) => Some(v),
                Err(_) => {
                    self.scanner_status = Some((true, "充电字节偏移必须为非负整数或留空".into()));
                    cx.notify();
                    return;
                }
            }
        };

        let sample = self
            .probe_result
            .as_ref()
            .filter(|r| r.ok)
            .map(|r| r.decoded_bytes.as_slice())
            .unwrap_or(&[]);

        let spec = build_hid_plugin_from_scan(
            &col,
            name,
            self.scanner_kind,
            self.probe_mode,
            req_hex,
            self.probe_xor_ff,
            level_offset,
            charging_offset,
            sample,
        );

        match save_plugin_to_user_dir(&spec) {
            Ok(path) => {
                self.reload_all_plugins_now(cx);
                let _ = self.poll_cmd_tx.send(PollCommand::ReloadPlugins);
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("plugin.json");
                self.scanner_status =
                    Some((false, format!("已保存插件 {fname} 并触发设备重扫")));
                if open_folder {
                    open_folder_in_explorer(&AppConfig::plugins_dir());
                }
            }
            Err(e) => {
                self.scanner_status = Some((true, format!("保存插件失败: {e}")));
            }
        }
        cx.notify();
    }

    pub fn create_bt_plugin_for_device(&mut self, bt: ScannedBtDevice, cx: &mut Context<Self>) {
        let spec = build_bt_plugin_from_scan(&bt, bt.kind);
        match save_plugin_to_user_dir(&spec) {
            Ok(path) => {
                self.reload_all_plugins_now(cx);
                let _ = self.poll_cmd_tx.send(PollCommand::ReloadPlugins);
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("bt_plugin.json");
                self.scanner_status = Some((
                    false,
                    format!("已生成蓝牙插件 {fname}，可在插件目录自定义 UUID/设备类型"),
                ));
            }
            Err(e) => {
                self.scanner_status = Some((true, format!("保存蓝牙插件失败: {e}")));
            }
        }
        cx.notify();
    }

    pub fn delete_loaded_plugin(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match delete_user_plugin(&path) {
            Ok(()) => {
                self.reload_all_plugins_now(cx);
                let _ = self.poll_cmd_tx.send(PollCommand::ReloadPlugins);
                self.scanner_status = Some((false, "已删除插件配置并重载".into()));
            }
            Err(e) => {
                self.scanner_status = Some((true, format!("删除插件失败: {e}")));
            }
        }
        cx.notify();
    }

    pub fn reload_all_plugins_now(&mut self, cx: &mut Context<Self>) {
        let (plugins, errors) = load_all_plugins();
        self.loaded_plugins = plugins;
        self.plugin_errors = errors;
        let _ = self.poll_cmd_tx.send(PollCommand::ReloadPlugins);
        cx.notify();
    }

    pub fn hide_to_tray(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.sync_window_position(window);
        self.visible = false;
        if let Some(h) = self.hwnd {
            cx.foreground_executor()
                .spawn(async move {
                    set_window_visible(h, false);
                })
                .detach();
        }
        self.sync_tray();
        cx.notify();
    }

    pub fn set_poll_interval(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.config.poll_interval_secs = secs;
        let _ = self.poll_cmd_tx.send(PollCommand::SetIntervalSecs(secs));
        self.save_config_if_persistent();
        cx.notify();
    }

    pub fn set_compact_max_per_row(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.compact_max_per_row = n.clamp(1, 6);
        self.save_config_if_persistent();
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn set_theme_mode(
        &mut self,
        pref: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.theme_mode = pref;
        let mode = match pref {
            ThemePreference::Light => ThemeMode::Light,
            ThemePreference::Dark | ThemePreference::System => ThemeMode::Dark,
        };
        Theme::change(mode, Some(window), cx);
        self.save_config_if_persistent();
        cx.notify();
    }

    pub fn toggle_show_sleeping(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.show_sleeping = !self.config.show_sleeping;
        self.save_config_if_persistent();
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn toggle_show_phones(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.show_phones = !self.config.show_phones;
        self.save_config_if_persistent();
        self.schedule_window_resize(window, cx, true);
        cx.notify();
    }

    pub fn toggle_low_battery_notify(&mut self, cx: &mut Context<Self>) {
        self.config.low_battery_notify = !self.config.low_battery_notify;
        self.save_config_if_persistent();
        cx.notify();
    }

    pub fn toggle_auto_start(&mut self, cx: &mut Context<Self>) {
        let next = !self.config.auto_start;
        if self.demo_mode || sync_autostart_registry(next).is_ok() {
            self.config.auto_start = next;
            self.save_config_if_persistent();
        }
        cx.notify();
    }

    fn format_updated_ago(&self) -> String {
        if self.polling && self.report.devices.is_empty() {
            return "扫描中...".to_string();
        }
        let secs = (Local::now() - self.report.polled_at).num_seconds().max(0);
        if self.polling {
            "更新中".to_string()
        } else if secs < 10 {
            "刚刚".to_string()
        } else if secs < 60 {
            format!("{secs}s前")
        } else {
            format!("{}m前", secs / 60)
        }
    }

    fn render_compact_bar(
        &self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let visible = self.visible_devices();
        let scale = window.scale_factor().max(1.0);
        let max_work_w = self
            .hwnd
            .and_then(get_monitor_work_area)
            .map(|wa| wa.width() as f32 / scale - 16.0)
            .unwrap_or(1200.0);
        let per_row = self.effective_compact_per_row(max_work_w);

        h_flex()
            .size_full()
            .items_center()
            .justify_between()
            .rounded(px(8.0))
            .bg(palette.window_bg)
            .border_1()
            .border_color(palette.window_border)
            .child(
                h_flex()
                    .id("compact-drag-area")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .pl_2()
                    .pr_1()
                    .py(px(7.0))
                    .items_center()
                    .gap_1p5()
                    .cursor(CursorStyle::OpenHand)
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::default()
                                    .path("icons/grip.svg")
                                    .size(px(12.0))
                                    .text_color(palette.text_muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .gap(px(6.0))
                            .when(visible.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(palette.text_secondary)
                                        .child(if self.polling {
                                            "正在检测设备..."
                                        } else {
                                            "无已连接设备"
                                        }),
                                )
                            })
                            .children(visible.chunks(per_row).enumerate().map(|(row_idx, chunk)| {
                                h_flex()
                                    .id(("compact-row", row_idx))
                                    .items_center()
                                    .gap(px(6.0))
                                    .children(chunk.iter().enumerate().map(|(ci, dev)| {
                                        let name =
                                            self.config.resolve_name(&dev.id, &dev.name).to_string();
                                        render_compact_chip(row_idx * per_row + ci, dev, name, palette)
                                    }))
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h_full()
                    .pr_2()
                    .items_center()
                    .gap_0p5()
                    .child(
                        Button::new("compact-refresh")
                            .ghost()
                            .xsmall()
                            .icon(
                                Icon::default()
                                    .path("icons/refresh.svg")
                                    .size(px(13.0))
                                    .text_color(if self.polling {
                                        palette.accent
                                    } else {
                                        palette.text_secondary
                                    }),
                            )
                            .tooltip("立即刷新")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.trigger_refresh(cx);
                            })),
                    )
                    .child(
                        Button::new("compact-expand")
                            .ghost()
                            .xsmall()
                            .icon(
                                Icon::default()
                                    .path("icons/expand.svg")
                                    .size(px(13.0))
                                    .text_color(palette.text_primary),
                            )
                            .tooltip("展开完整卡片")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_compact_mode(window, cx);
                            })),
                    ),
            )
    }
}

impl Render for BatteryWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = match self.config.theme_mode {
            ThemePreference::Dark => true,
            ThemePreference::Light => false,
            ThemePreference::System => cx.theme().mode.is_dark(),
        };
        let palette = Palette::from_dark(is_dark);

        if self.config.compact_mode {
            return self
                .render_compact_bar(&palette, window, cx)
                .into_any_element();
        }

        let visible = self.visible_devices();
        let active_count = visible.iter().filter(|d| !d.power_state.is_inactive()).count();
        let status_dot_color = if self.polling {
            palette.amber
        } else if active_count > 0 {
            palette.emerald
        } else {
            palette.slate
        };

        let updated_label = self.format_updated_ago();

        v_flex()
            .size_full()
            .rounded(px(8.0))
            .bg(palette.window_bg)
            .border_1()
            .border_color(palette.window_border)
            .overflow_hidden()
            // Header (drag handle + action buttons)
            .child(
                h_flex()
                    .w_full()
                    .h(px(42.0))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .bg(palette.header_bg)
                    .border_b_1()
                    .border_color(palette.divider)
                    .child(
                        h_flex()
                            .id("drag-area")
                            .flex_1()
                            .h_full()
                            .items_center()
                            .gap_2()
                            .cursor(CursorStyle::OpenHand)
                            .window_control_area(WindowControlArea::Drag)
                            .child(
                                div()
                                    .size(px(8.0))
                                    .rounded_full()
                                    .bg(status_dot_color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(palette.text_primary)
                                    .child("设备电量"),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.5))
                                    .rounded(px(4.0))
                                    .bg(palette.pill_bg)
                                    .text_size(px(10.0))
                                    .text_color(palette.text_secondary)
                                    .child(updated_label),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_0p5()
                            .child(
                                Button::new("btn-refresh")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path("icons/refresh.svg")
                                            .size(px(13.5))
                                            .text_color(if self.polling {
                                                palette.accent
                                            } else {
                                                palette.text_secondary
                                            }),
                                    )
                                    .tooltip("立即刷新")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.trigger_refresh(cx);
                                    })),
                            )
                            .child(
                                Button::new("btn-compact")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path("icons/compact.svg")
                                            .size(px(13.5))
                                            .text_color(palette.text_secondary),
                                    )
                                    .tooltip("精简悬浮条")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_compact_mode(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("btn-pin")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path(if self.config.always_on_top {
                                                "icons/pin.svg"
                                            } else {
                                                "icons/pin-off.svg"
                                            })
                                            .size(px(13.5))
                                            .text_color(if self.config.always_on_top {
                                                palette.accent
                                            } else {
                                                palette.text_muted
                                            }),
                                    )
                                    .tooltip(if self.config.always_on_top {
                                        "取消置顶"
                                    } else {
                                        "始终置顶"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_always_on_top(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("btn-scanner")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path("icons/scan.svg")
                                            .size(px(13.5))
                                            .text_color(if self.show_scanner {
                                                palette.accent
                                            } else {
                                                palette.text_secondary
                                            }),
                                    )
                                    .tooltip("设备扫描与自定义插件")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_scanner(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("btn-settings")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path("icons/settings.svg")
                                            .size(px(13.5))
                                            .text_color(if self.show_settings {
                                                palette.accent
                                            } else {
                                                palette.text_secondary
                                            }),
                                    )
                                    .tooltip("设置与诊断")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_settings(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("btn-hide")
                                    .ghost()
                                    .xsmall()
                                    .icon(
                                        Icon::default()
                                            .path("icons/minimize.svg")
                                            .size(px(13.5))
                                            .text_color(palette.text_secondary),
                                    )
                                    .tooltip("收起到系统托盘")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.hide_to_tray(window, cx);
                                    })),
                            ),
                    ),
            )
            // Device list container
            .child(
                v_flex()
                    .id("device-list")
                    .w_full()
                    .flex_1()
                    .p_2p5()
                    .gap_2()
                    .overflow_y_scroll()
                    .when(visible.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .py_3()
                                .gap_1p5()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette.text_secondary)
                                        .child(if self.polling {
                                            "正在检测已连接的无线设备..."
                                        } else {
                                            "暂无已连接的无线设备"
                                        }),
                                )
                                .when(!self.polling, |inner| {
                                    inner.child(
                                        h_flex()
                                            .gap_1p5()
                                            .child(
                                                Button::new("btn-rescan")
                                                    .secondary()
                                                    .xsmall()
                                                    .label("重新刷新")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.trigger_refresh(cx);
                                                    })),
                                            )
                                            .when(!self.show_scanner, |b| {
                                                b.child(
                                                    Button::new("btn-empty-scan")
                                                        .ghost()
                                                        .xsmall()
                                                        .label("扫描设备")
                                                        .on_click(cx.listener(|this, _, window, cx| {
                                                            this.toggle_scanner(window, cx);
                                                        })),
                                                )
                                            }),
                                    )
                                }),
                        )
                    })
                    .children(visible.into_iter().enumerate().map(|(i, dev)| {
                        let name = self.config.resolve_name(&dev.id, &dev.name).to_string();
                        render_device_card(
                            i,
                            dev,
                            name,
                            self.config.low_battery_threshold,
                            &palette,
                        )
                    })),
            )
            // Collapsible Settings & Diagnostics Drawer
            .when(self.show_settings, |this| {
                this.child(render_settings_panel(
                    &self.config,
                    self.show_diagnostics,
                    &self.report.diagnostics,
                    &palette,
                    cx,
                ))
            })
            // Collapsible Device Scanner & Plugin Manager Drawer
            .when(self.show_scanner, |this| {
                this.child(render_scanner_panel(self, &palette, cx))
            })
            .into_any_element()
    }
}
