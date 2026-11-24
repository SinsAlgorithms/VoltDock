#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod assets;
mod config;
mod manager;
mod model;
mod platform_win;
mod plugins;
mod providers;
mod transport;
mod ui;

use std::path::PathBuf;

use gpui_kit::*;

use crate::assets::AppAssets;
use crate::config::{AppConfig, install_panic_hook, sync_autostart_registry};
use crate::manager::{run_single_poll, run_test_single_plugin, spawn_poll_worker};
use crate::platform_win::{
    SingleInstanceGuard, TrayController, TraySyncState, attach_parent_console,
    default_widget_position, get_monitor_work_area_for_point, get_scale_factor_for_point,
    wake_existing_instance, write_cli_output,
};
use crate::plugins::runner::{
    ensure_plugins_dir_and_examples, load_all_plugins, load_plugin_file, plugin_search_dirs,
};
use crate::plugins::scanner::run_device_scan;
use crate::ui::widget::{BatteryWidget, CARD_WIDTH, DemoOptions, SCANNER_WIDTH};

fn print_help() {
    let msg = "\
VoltDock - 无线外设电量桌面小组件 (2.4GHz + 蓝牙 + 插件化扩展)

用法:
  voltdock                       启动桌面悬浮小组件（若已在运行则唤醒置前）
  voltdock --probe               扫描并打印当前所有检测到的设备电量及诊断日志
  voltdock --probe-json          以 JSON 格式输出探测结果
  voltdock --scan                扫描本机所有 USB HID 接口属性与蓝牙设备列表
  voltdock --plugins             列出插件搜索目录与已加载的自定义插件
  voltdock --test-plugin PATH    单独测试指定路径的插件 JSON 配置文件
  voltdock --demo                以内置示例数据预览 UI（不写入本地配置）
  voltdock --demo-view VIEW      指定预览视图: card-dark | card-light | compact | settings | scanner
  voltdock --help                显示帮助
";
    write_cli_output(msg);
}

fn format_poll_report(report: &crate::model::PollReport, title: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "=== {title} ({}) ===\n",
        report.polled_at.format("%Y-%m-%d %H:%M:%S")
    ));
    if report.devices.is_empty() {
        out.push_str("未检测到支持电量上报的无线外设。\n");
    } else {
        for d in &report.devices {
            let vp = d
                .vid_pid
                .map(|(v, p)| format!(" [{v:04X}:{p:04X}]"))
                .unwrap_or_default();
            let mac = d
                .mac_address
                .as_ref()
                .map(|m| format!(" [MAC:{m}]"))
                .unwrap_or_default();
            let detail = d
                .detail
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {:<24} | {:<4} | {:<6} | {:<9} | {} ({}){}{}{}\n",
                d.name,
                d.display_percent(),
                d.connection.badge_label(),
                d.power_state.label_zh(),
                d.kind.label_zh(),
                d.provider,
                vp,
                mac,
                detail
            ));
        }
    }
    out.push_str(&format!(
        "\n--- 诊断日志 ({} 条) ---\n",
        report.diagnostics.len()
    ));
    for line in &report.diagnostics {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn run_probe(json_mode: bool) {
    attach_parent_console();
    let report = run_single_poll();
    if json_mode {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => write_cli_output(&format!("{s}\n")),
            Err(e) => eprintln!("JSON 序列化失败: {e}"),
        }
        return;
    }
    write_cli_output(&format_poll_report(&report, "设备电量探测"));
}

fn run_scan_cli() {
    attach_parent_console();
    let (plugins, _) = load_all_plugins();
    let snap = run_device_scan(&plugins);
    let mut out = String::new();
    out.push_str(&format!(
        "=== 硬件与驱动属性扫描 ({}) ===\n",
        snap.scanned_at
    ));
    out.push_str(&format!("\n[USB HID 设备组: {} 组]\n", snap.hid_groups.len()));
    for g in &snap.hid_groups {
        out.push_str(&format!(
            "* {:04X}:{:04X} | {} | 厂商='{}' | 状态={}{}\n",
            g.vid,
            g.pid,
            g.display_name,
            g.manufacturer,
            g.support.label(),
            if g.has_vendor_channel { " [含厂商通道]" } else { "" }
        ));
        for c in &g.collections {
            let mi = c
                .interface_number
                .map(|m| format!("MI_{m:02X}"))
                .unwrap_or_else(|| "MI_--".to_string());
            out.push_str(&format!(
                "    - {mi} | UP={:04X} U={:04X} | In={} Out={} Feat={} | {}{}\n",
                c.usage_page,
                c.usage,
                c.input_len,
                c.output_len,
                c.feature_len,
                c.support.label(),
                if c.is_vendor_channel { " [Vendor]" } else { "" }
            ));
        }
    }

    out.push_str(&format!("\n[蓝牙设备: {} 个]\n", snap.bt_devices.len()));
    for b in &snap.bt_devices {
        out.push_str(&format!(
            "* {} | MAC={} | {} | 已连接={} | 电量={:?} | 类型={:?} | 状态={}\n",
            b.name,
            b.mac,
            if b.is_le && b.has_classic {
                "BLE+Classic"
            } else if b.is_le {
                "BLE"
            } else {
                "Classic"
            },
            b.connected,
            b.battery_percent,
            b.kind,
            b.support.label()
        ));
    }
    write_cli_output(&out);
}

fn run_plugins_cli() {
    attach_parent_console();
    let (plugins, errors) = load_all_plugins();
    let mut out = String::from("=== 插件搜索目录 ===\n");
    for d in plugin_search_dirs() {
        out.push_str(&format!("- {}\n", d.display()));
    }
    out.push_str(&format!("\n=== 已加载插件 ({} 个) ===\n", plugins.len()));
    for p in &plugins {
        out.push_str(&format!(
            "- [{}] {} (enabled={}, priority={}, transport={:?}) <- {}\n",
            p.spec.id,
            p.spec.name,
            p.spec.enabled,
            p.spec.priority,
            p.spec.transport,
            p.path.display()
        ));
    }
    if !errors.is_empty() {
        out.push_str(&format!("\n=== 加载错误 ({} 个) ===\n", errors.len()));
        for e in &errors {
            out.push_str(&format!("- {}: {}\n", e.path.display(), e.error));
        }
    }
    write_cli_output(&out);
}

fn run_test_plugin_cli(path_str: &str) {
    attach_parent_console();
    let path = PathBuf::from(path_str);
    match load_plugin_file(&path, false) {
        Ok(loaded) => {
            let report = run_test_single_plugin(loaded);
            write_cli_output(&format_poll_report(&report, "单插件测试结果"));
        }
        Err(e) => {
            write_cli_output(&format!("加载插件失败 {}: {e}\n", path.display()));
        }
    }
}

fn main() {
    install_panic_hook();
    ensure_plugins_dir_and_examples();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        attach_parent_console();
        print_help();
        return;
    }
    if args.iter().any(|a| a == "--probe-json") {
        run_probe(true);
        return;
    }
    if args.iter().any(|a| a == "--probe") {
        run_probe(false);
        return;
    }
    if args.iter().any(|a| a == "--scan" || a == "--scan-hid") {
        run_scan_cli();
        return;
    }
    if args.iter().any(|a| a == "--plugins") {
        run_plugins_cli();
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "--test-plugin") {
        if let Some(path) = args.get(pos + 1) {
            run_test_plugin_cli(path);
        } else {
            attach_parent_console();
            write_cli_output("用法: voltdock --test-plugin <插件JSON文件路径>\n");
        }
        return;
    }

    let demo_view = args
        .iter()
        .position(|a| a == "--demo-view")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.as_str());
    let demo_enabled = args.iter().any(|a| a == "--demo") || demo_view.is_some();
    let demo = if demo_enabled {
        match demo_view.unwrap_or("card-dark") {
            "card-light" => DemoOptions {
                enabled: true,
                light_theme: true,
                ..Default::default()
            },
            "compact" => DemoOptions {
                enabled: true,
                compact: true,
                compact_max_per_row: 2,
                ..Default::default()
            },
            "settings" => DemoOptions {
                enabled: true,
                open_settings: true,
                ..Default::default()
            },
            "scanner" => DemoOptions {
                enabled: true,
                open_scanner: true,
                ..Default::default()
            },
            _ => DemoOptions {
                enabled: true,
                ..Default::default()
            },
        }
    } else {
        DemoOptions::default()
    };

    let _instance_guard = if demo.enabled {
        None
    } else {
        let guard = SingleInstanceGuard::acquire();
        if !guard.is_primary {
            let _ = wake_existing_instance();
            return;
        }
        Some(guard)
    };

    let config = AppConfig::load();
    if !demo.enabled && config.auto_start {
        let _ = sync_autostart_registry(true);
    }

    let (initial_logical_w, initial_logical_h) = if demo.enabled {
        if demo.open_scanner {
            (SCANNER_WIDTH as i32, 620)
        } else if demo.open_settings {
            (CARD_WIDTH as i32, 560)
        } else if demo.compact {
            (260, 76)
        } else {
            (CARD_WIDTH as i32, 340)
        }
    } else {
        (
            if config.compact_mode {
                196
            } else {
                CARD_WIDTH as i32
            },
            if config.compact_mode { 42 } else { 126 },
        )
    };

    let (est_x, est_y) = if demo.enabled {
        let sys_scale = get_scale_factor_for_point(0, 0);
        default_widget_position(
            (initial_logical_w as f32 * sys_scale).round() as i32,
            (initial_logical_h as f32 * sys_scale).round() as i32,
        )
    } else {
        match (config.window_x, config.window_y) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                let sys_scale = get_scale_factor_for_point(0, 0);
                default_widget_position(
                    (initial_logical_w as f32 * sys_scale).round() as i32,
                    (initial_logical_h as f32 * sys_scale).round() as i32,
                )
            }
        }
    };
    let wa = get_monitor_work_area_for_point(est_x, est_y);
    let scale = get_scale_factor_for_point(est_x, est_y);
    let est_phys_w = (initial_logical_w as f32 * scale).round() as i32;
    let est_phys_h = (initial_logical_h as f32 * scale).round() as i32;
    let win_x = est_x.clamp(wa.left, (wa.right - est_phys_w).max(wa.left));
    let win_y = est_y.clamp(wa.top, (wa.bottom - est_phys_h).max(wa.top));

    let (poll_cmd_tx, report_rx) = if demo.enabled {
        let (tx, _rx) = std::sync::mpsc::channel();
        let (rep_tx, rep_rx) = smol::channel::unbounded();
        drop(rep_tx);
        (tx, rep_rx)
    } else {
        spawn_poll_worker(config.poll_interval_secs)
    };

    #[cfg(target_os = "windows")]
    let (tray_ctrl, tray_rx) = if demo.enabled {
        (None, None)
    } else {
        let (ctrl, rx) = TrayController::start(TraySyncState {
            visible: true,
            always_on_top: config.always_on_top,
            compact_mode: config.compact_mode,
        });
        (Some(ctrl), Some(rx))
    };
    #[cfg(not(target_os = "windows"))]
    let (tray_ctrl, tray_rx) = (None, None);

    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds {
                origin: point(px(win_x as f32 / scale), px(win_y as f32 / scale)),
                size: size(px(initial_logical_w as f32), px(initial_logical_h as f32)),
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                kind: WindowKind::PopUp,
                is_movable: true,
                is_resizable: false,
                is_minimizable: false,
                focus: false,
                show: true,
                window_background: if config.glass_backdrop {
                    WindowBackgroundAppearance::Blurred
                } else {
                    WindowBackgroundAppearance::Transparent
                },
                ..Default::default()
            };

            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    BatteryWidget::new(
                        config,
                        (win_x, win_y),
                        poll_cmd_tx,
                        report_rx,
                        tray_ctrl,
                        tray_rx,
                        demo,
                        window,
                        cx,
                    )
                })
            })
            .expect("Failed to open VoltDock widget window");
        });
}
