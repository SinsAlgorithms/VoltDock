use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::AppConfig;
use crate::model::DeviceKind;
use crate::platform_win::open_folder_in_explorer;
use crate::plugins::scanner::SupportStatus;
use crate::plugins::schema::{HidProtocolMode, PluginTransport};
use crate::ui::Palette;
use crate::ui::widget::BatteryWidget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScannerTab {
    #[default]
    Hid,
    Bluetooth,
    Plugins,
}

fn support_colors(support: &SupportStatus, palette: &Palette) -> (Hsla, Hsla) {
    match support {
        SupportStatus::Plugin(_) => (palette.cyan.opacity(0.18), palette.cyan),
        SupportStatus::BuiltIn(_) => (palette.emerald.opacity(0.18), palette.emerald),
        SupportStatus::Unmatched => (palette.pill_bg, palette.text_muted),
    }
}

pub fn render_scanner_panel(
    widget: &BatteryWidget,
    palette: &Palette,
    cx: &mut Context<BatteryWidget>,
) -> impl IntoElement {
    let tabs: &[(ScannerTab, &str)] = &[
        (ScannerTab::Hid, "USB/HID 扫描"),
        (ScannerTab::Bluetooth, "蓝牙扫描"),
        (ScannerTab::Plugins, "已加载插件"),
    ];

    let hid_modes: &[(HidProtocolMode, &str)] = &[
        (HidProtocolMode::FeatureReport, "Feature"),
        (HidProtocolMode::OutputInput, "Out+In"),
        (HidProtocolMode::PassiveInput, "被动读取"),
    ];

    let kind_choices: &[(DeviceKind, &str)] = &[
        (DeviceKind::Mouse, "鼠标"),
        (DeviceKind::Headset, "耳机"),
        (DeviceKind::Gamepad, "手柄"),
        (DeviceKind::Keyboard, "键盘"),
        (DeviceKind::Other, "其他"),
    ];

    let plugin_count = widget.loaded_plugins.len();

    v_flex()
        .w_full()
        .px_2p5()
        .py_2()
        .gap_2()
        .border_t_1()
        .border_color(palette.divider)
        // Top Tab Strip + Utility buttons
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_1()
                .child(h_flex().gap_1().children(tabs.iter().enumerate().map(|(i, &(tab, lbl))| {
                    let active = widget.scanner_tab == tab;
                    let full_label = if tab == ScannerTab::Plugins {
                        format!("{lbl} ({plugin_count})")
                    } else {
                        lbl.to_string()
                    };
                    div()
                        .id(("scan-tab", i))
                        .cursor_pointer()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .text_size(px(11.0))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .bg(if active {
                            palette.accent.opacity(0.22)
                        } else {
                            palette.pill_bg
                        })
                        .text_color(if active {
                            palette.accent
                        } else {
                            palette.text_secondary
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_scanner_tab(tab, window, cx);
                        }))
                        .child(full_label)
                })))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("scan-refresh-btn")
                                .ghost()
                                .xsmall()
                                .icon(
                                    Icon::default()
                                        .path("icons/refresh.svg")
                                        .size(px(12.0))
                                        .text_color(if widget.scanning_devices {
                                            palette.accent
                                        } else {
                                            palette.text_secondary
                                        }),
                                )
                                .tooltip("重新扫描硬件与插件")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.trigger_device_scan(cx);
                                    this.reload_all_plugins_now(cx);
                                })),
                        )
                        .child(
                            Button::new("scan-folder-btn")
                                .ghost()
                                .xsmall()
                                .icon(
                                    Icon::default()
                                        .path("icons/folder.svg")
                                        .size(px(12.0))
                                        .text_color(palette.text_secondary),
                                )
                                .tooltip("打开插件配置目录")
                                .on_click(|_, _, _| {
                                    open_folder_in_explorer(&AppConfig::plugins_dir());
                                }),
                        ),
                ),
        )
        // Optional feedback banner
        .when_some(widget.scanner_status.as_ref(), |this, (is_err, msg)| {
            this.child(
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .bg(if *is_err {
                        palette.rose.opacity(0.16)
                    } else {
                        palette.emerald.opacity(0.16)
                    })
                    .text_size(px(10.5))
                    .text_color(if *is_err { palette.rose } else { palette.emerald })
                    .child(msg.clone()),
            )
        })
        // ============================================================
        // TAB 1: USB/HID Scanner & Probe Generator
        // ============================================================
        .when(widget.scanner_tab == ScannerTab::Hid, |this| {
            let vendor_only = widget.scanner_vendor_only;
            let selected_path = widget
                .selected_hid_col
                .as_ref()
                .map(|c| c.path.as_str())
                .unwrap_or("");

            // Flatten groups & matching collections for scrollable list
            let mut rows = Vec::new();
            for grp in &widget.scan_snapshot.hid_groups {
                if vendor_only && !grp.has_vendor_channel {
                    continue;
                }
                let cols: Vec<_> = grp
                    .collections
                    .iter()
                    .filter(|c| !vendor_only || c.is_vendor_channel)
                    .cloned()
                    .collect();
                if !cols.is_empty() {
                    rows.push((grp.clone(), cols));
                }
            }

            this.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(palette.text_muted)
                            .child(format!(
                                "扫描时间 {} · {} 组 HID 设备",
                                if widget.scan_snapshot.scanned_at.is_empty() {
                                    "--:--"
                                } else {
                                    widget.scan_snapshot.scanned_at.as_str()
                                },
                                rows.len()
                            )),
                    )
                    .child(
                        div()
                            .id("scan-vendor-filter")
                            .cursor_pointer()
                            .px_1p5()
                            .py(px(1.5))
                            .rounded(px(4.0))
                            .text_size(px(10.0))
                            .bg(if vendor_only {
                                palette.accent.opacity(0.20)
                            } else {
                                palette.pill_bg
                            })
                            .text_color(if vendor_only {
                                palette.accent
                            } else {
                                palette.text_secondary
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_vendor_only_filter(cx);
                            }))
                            .child(if vendor_only {
                                "仅厂商通道 (0xFFxx)"
                            } else {
                                "显示全部 HID"
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("hid-scan-list")
                    .w_full()
                    .h(px(138.0))
                    .p_1p5()
                    .gap_1()
                    .rounded(px(7.0))
                    .bg(palette.card_bg)
                    .border_1()
                    .border_color(palette.card_border)
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |box_el| {
                        box_el.child(
                            div()
                                .py_4()
                                .text_center()
                                .text_xs()
                                .text_color(palette.text_muted)
                                .child(if widget.scanning_devices {
                                    "正在扫描 USB HID 驱动属性..."
                                } else {
                                    "暂无匹配 HID 接口，可点击“显示全部 HID”或刷新"
                                }),
                        )
                    })
                    .children(rows.into_iter().enumerate().map(|(gi, (grp, cols))| {
                        let (badge_bg, badge_fg) = support_colors(&grp.support, palette);
                        v_flex()
                            .id(("hid-grp", gi))
                            .w_full()
                            .p_1p5()
                            .gap_1()
                            .rounded(px(6.0))
                            .bg(palette.header_bg)
                            .border_1()
                            .border_color(palette.card_border)
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex_1()
                                            .truncate()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(palette.text_primary)
                                            .child(format!(
                                                "{} [{:04X}:{:04X}]",
                                                grp.display_name, grp.vid, grp.pid
                                            )),
                                    )
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(badge_bg)
                                            .text_size(px(9.5))
                                            .text_color(badge_fg)
                                            .child(grp.support.label()),
                                    ),
                            )
                            .children(cols.into_iter().enumerate().map(|(ci, col)| {
                                let is_sel = col.path == selected_path;
                                let mi_txt = col
                                    .interface_number
                                    .map(|m| format!("MI_{m:02X}"))
                                    .unwrap_or_else(|| "MI_--".to_string());
                                let col_for_click = col.clone();
                                h_flex()
                                    .id(("hid-col", gi * 100 + ci))
                                    .w_full()
                                    .cursor_pointer()
                                    .items_center()
                                    .justify_between()
                                    .px_1p5()
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(if is_sel {
                                        palette.accent.opacity(0.20)
                                    } else {
                                        palette.card_bg.opacity(0.6)
                                    })
                                    .border_1()
                                    .border_color(if is_sel {
                                        palette.accent
                                    } else {
                                        rgba(0x00000000).into()
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_hid_collection(col_for_click.clone(), window, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(if is_sel {
                                                palette.text_primary
                                            } else {
                                                palette.text_secondary
                                            })
                                            .child(format!(
                                                "{mi_txt}  UP={:04X} U={:04X}  I:{} O:{} F:{}",
                                                col.usage_page,
                                                col.usage,
                                                col.input_len,
                                                col.output_len,
                                                col.feature_len
                                            )),
                                    )
                                    .when(col.is_vendor_channel, |el| {
                                        el.child(
                                            div()
                                                .px_1()
                                                .rounded(px(3.0))
                                                .bg(palette.cyan.opacity(0.16))
                                                .text_size(px(9.0))
                                                .text_color(palette.cyan)
                                                .child("厂商通道"),
                                        )
                                    })
                            }))
                    })),
            )
            // Probe & Quick Plugin Builder for selected HID interface
            .when_some(widget.selected_hid_col.as_ref(), |outer, sel| {
                outer.child(
                    v_flex()
                        .w_full()
                        .p_2()
                        .gap_1p5()
                        .rounded(px(7.0))
                        .bg(palette.card_bg)
                        .border_1()
                        .border_color(palette.card_border)
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.text_primary)
                                        .child(format!(
                                            "协议探测 [{:04X}:{:04X} · UP={:04X}]",
                                            sel.vid, sel.pid, sel.usage_page
                                        )),
                                )
                                .child(h_flex().gap_1().children(hid_modes.iter().enumerate().map(|(i, &(m, lbl))| {
                                    let active = widget.probe_mode == m;
                                    div()
                                        .id(("pmode", i))
                                        .cursor_pointer()
                                        .px_1p5()
                                        .py(px(1.5))
                                        .rounded(px(4.0))
                                        .text_size(px(10.0))
                                        .bg(if active {
                                            palette.accent.opacity(0.22)
                                        } else {
                                            palette.pill_bg
                                        })
                                        .text_color(if active {
                                            palette.accent
                                        } else {
                                            palette.text_secondary
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_probe_mode(m, cx);
                                        }))
                                        .child(lbl)
                                }))),
                        )
                        // Probe command input + XOR 0xFF toggle + Send button
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Input::new(&widget.input_probe_hex).xsmall()),
                                )
                                .child(
                                    div()
                                        .id("btn-xor-toggle")
                                        .cursor_pointer()
                                        .px_1p5()
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .text_size(px(10.0))
                                        .bg(if widget.probe_xor_ff {
                                            palette.cyan.opacity(0.22)
                                        } else {
                                            palette.pill_bg
                                        })
                                        .text_color(if widget.probe_xor_ff {
                                            palette.cyan
                                        } else {
                                            palette.text_secondary
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_probe_xor(cx);
                                        }))
                                        .child("XOR FF"),
                                )
                                .child(
                                    Button::new("btn-send-probe")
                                        .secondary()
                                        .xsmall()
                                        .disabled(widget.probing_hid)
                                        .icon(
                                            Icon::default()
                                                .path("icons/play.svg")
                                                .size(px(11.0))
                                                .text_color(palette.accent),
                                        )
                                        .label(if widget.probing_hid { "探测中" } else { "试发读取" })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.trigger_hid_probe(cx);
                                        })),
                                ),
                        )
                        // Probe response & clickable byte offset pills
                        .when_some(widget.probe_result.as_ref(), |box_el, res| {
                            box_el
                                .child(
                                    div()
                                        .w_full()
                                        .text_size(px(10.0))
                                        .text_color(if res.ok {
                                            palette.emerald
                                        } else {
                                            palette.amber
                                        })
                                        .child(res.message.clone()),
                                )
                                .when(res.ok, |ok_el| {
                                    ok_el
                                        .child(
                                            div()
                                                .w_full()
                                                .p_1()
                                                .rounded(px(4.0))
                                                .bg(palette.header_bg)
                                                .text_size(px(9.5))
                                                .text_color(palette.text_secondary)
                                                .child(if res.raw_hex != res.decoded_hex {
                                                    format!(
                                                        "解码: {}\n原始: {}",
                                                        res.decoded_hex, res.raw_hex
                                                    )
                                                } else {
                                                    res.decoded_hex.clone()
                                                }),
                                        )
                                        .when(!res.candidate_offsets.is_empty(), |c_el| {
                                            c_el.child(
                                                h_flex()
                                                    .w_full()
                                                    .flex_wrap()
                                                    .gap_1()
                                                    .children(res.candidate_offsets.iter().take(10).map(|&(idx, val)| {
                                                        div()
                                                            .id(("cand-off", idx))
                                                            .cursor_pointer()
                                                            .px_1p5()
                                                            .py(px(1.5))
                                                            .rounded(px(4.0))
                                                            .bg(palette.accent.opacity(0.16))
                                                            .text_size(px(9.5))
                                                            .text_color(palette.accent)
                                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                                this.apply_candidate_offset(idx, window, cx);
                                                            }))
                                                            .child(format!("[{idx}]={val}%"))
                                                    })),
                                            )
                                        })
                                })
                        })
                        // Quick Plugin Form: Name + Kind + Level/Charging offsets
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Input::new(&widget.input_plugin_name).xsmall()),
                                )
                                .child(
                                    h_flex().gap(px(2.0)).children(kind_choices.iter().enumerate().map(|(i, &(k, lbl))| {
                                        let active = widget.scanner_kind == k;
                                        div()
                                            .id(("skind", i))
                                            .cursor_pointer()
                                            .px_1p5()
                                            .py(px(2.5))
                                            .rounded(px(4.0))
                                            .text_size(px(10.0))
                                            .bg(if active {
                                                palette.accent.opacity(0.22)
                                            } else {
                                                palette.pill_bg
                                            })
                                            .text_color(if active {
                                                palette.accent
                                            } else {
                                                palette.text_secondary
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_scanner_kind(k, cx);
                                            }))
                                            .child(lbl)
                                    })),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(palette.text_secondary)
                                        .child("电量字节:"),
                                )
                                .child(
                                    div()
                                        .w(px(54.0))
                                        .child(Input::new(&widget.input_level_offset).xsmall()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(palette.text_secondary)
                                        .child("充电字节:"),
                                )
                                .child(
                                    div()
                                        .w(px(54.0))
                                        .child(Input::new(&widget.input_charging_offset).xsmall()),
                                )
                                .child(
                                    Button::new("btn-save-hid-plugin")
                                        .primary()
                                        .xsmall()
                                        .label("保存插件并生效")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_scanned_hid_plugin(false, cx);
                                        })),
                                ),
                        ),
                )
            })
        })
        // ============================================================
        // TAB 2: Bluetooth Devices Scanner
        // ============================================================
        .when(widget.scanner_tab == ScannerTab::Bluetooth, |this| {
            this.child(
                v_flex()
                    .id("bt-scan-list")
                    .w_full()
                    .h(px(240.0))
                    .p_1p5()
                    .gap_1p5()
                    .rounded(px(7.0))
                    .bg(palette.card_bg)
                    .border_1()
                    .border_color(palette.card_border)
                    .overflow_y_scroll()
                    .when(widget.scan_snapshot.bt_devices.is_empty(), |box_el| {
                        box_el.child(
                            div()
                                .py_6()
                                .text_center()
                                .text_xs()
                                .text_color(palette.text_muted)
                                .child("未扫描到已配对蓝牙设备，点击右上角刷新重试"),
                        )
                    })
                    .children(widget.scan_snapshot.bt_devices.iter().enumerate().map(|(i, bt)| {
                        let (badge_bg, badge_fg) = support_colors(&bt.support, palette);
                        let bt_clone = bt.clone();
                        h_flex()
                            .id(("bt-row", i))
                            .w_full()
                            .p_1p5()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .rounded(px(6.0))
                            .bg(palette.header_bg)
                            .border_1()
                            .border_color(palette.card_border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.0))
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(palette.text_primary)
                                                    .child(bt.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .px_1()
                                                    .rounded(px(3.0))
                                                    .bg(if bt.connected {
                                                        palette.emerald.opacity(0.18)
                                                    } else {
                                                        palette.pill_bg
                                                    })
                                                    .text_size(px(9.5))
                                                    .text_color(if bt.connected {
                                                        palette.emerald
                                                    } else {
                                                        palette.text_muted
                                                    })
                                                    .child(if bt.connected { "已连接" } else { "未连接" }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(palette.text_secondary)
                                            .child(format!(
                                                "MAC:{} · {} · 电量:{}",
                                                bt.mac,
                                                if bt.is_le && bt.has_classic {
                                                    "BLE+Classic"
                                                } else if bt.is_le {
                                                    "BLE"
                                                } else {
                                                    "Classic"
                                                },
                                                bt.battery_percent
                                                    .map(|p| format!("{p}%"))
                                                    .unwrap_or_else(|| "无/未读到".into())
                                            )),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py(px(1.5))
                                            .rounded(px(4.0))
                                            .bg(badge_bg)
                                            .text_size(px(9.5))
                                            .text_color(badge_fg)
                                            .child(bt.support.label()),
                                    )
                                    .child(
                                        Button::new(("bt-add-plugin", i))
                                            .secondary()
                                            .xsmall()
                                            .label("生成插件")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.create_bt_plugin_for_device(bt_clone.clone(), cx);
                                            })),
                                    ),
                            )
                    })),
            )
        })
        // ============================================================
        // TAB 3: Loaded Plugins Manager
        // ============================================================
        .when(widget.scanner_tab == ScannerTab::Plugins, |this| {
            this.child(
                v_flex()
                    .id("loaded-plugins-list")
                    .w_full()
                    .h(px(240.0))
                    .p_1p5()
                    .gap_1p5()
                    .rounded(px(7.0))
                    .bg(palette.card_bg)
                    .border_1()
                    .border_color(palette.card_border)
                    .overflow_y_scroll()
                    .when(!widget.plugin_errors.is_empty(), |box_el| {
                        box_el.children(widget.plugin_errors.iter().enumerate().map(|(i, err)| {
                            div()
                                .id(("plg-err", i))
                                .w_full()
                                .p_1p5()
                                .rounded(px(5.0))
                                .bg(palette.rose.opacity(0.15))
                                .text_size(px(10.0))
                                .text_color(palette.rose)
                                .child(format!("解析失败 {:?}: {}", err.path.file_name().unwrap_or_default(), err.error))
                        }))
                    })
                    .when(widget.loaded_plugins.is_empty() && widget.plugin_errors.is_empty(), |box_el| {
                        box_el.child(
                            v_flex()
                                .py_6()
                                .gap_1()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette.text_secondary)
                                        .child("当前未启用任何 .json 自定义设备插件"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(palette.text_muted)
                                        .child("可在“USB/HID 扫描”快速保存，或将 plugins\\examples 模板改为 .json"),
                                ),
                        )
                    })
                    .children(widget.loaded_plugins.iter().enumerate().map(|(i, lp)| {
                        let match_summary = match lp.spec.transport {
                            PluginTransport::Hid => format!(
                                "HID VID={}{}",
                                lp.spec
                                    .matcher
                                    .vid
                                    .map(|v| format!("{v:04X}"))
                                    .unwrap_or_else(|| "*".into()),
                                lp.spec
                                    .matcher
                                    .usage_page
                                    .map(|u| format!(" UP={u:04X}"))
                                    .unwrap_or_default()
                            ),
                            PluginTransport::Bluetooth => format!(
                                "BT name='{}'",
                                lp.spec
                                    .matcher
                                    .name_contains
                                    .as_deref()
                                    .unwrap_or("*")
                            ),
                        };
                        let path_for_del = lp.path.clone();
                        let is_user = lp.is_user_dir;
                        h_flex()
                            .id(("plg-item", i))
                            .w_full()
                            .p_1p5()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .rounded(px(6.0))
                            .bg(palette.header_bg)
                            .border_1()
                            .border_color(palette.card_border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.0))
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                Icon::default()
                                                    .path("icons/plug.svg")
                                                    .size(px(12.0))
                                                    .text_color(palette.cyan),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(palette.text_primary)
                                                    .child(lp.spec.name.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(10.0))
                                            .text_color(palette.text_secondary)
                                            .child(format!(
                                                "{} · {match_summary} · {:?}",
                                                lp.spec.id,
                                                lp.path.file_name().unwrap_or_default()
                                            )),
                                    ),
                            )
                            .when(is_user, |el| {
                                el.child(
                                    Button::new(("plg-del", i))
                                        .ghost()
                                        .xsmall()
                                        .icon(
                                            Icon::default()
                                                .path("icons/trash.svg")
                                                .size(px(12.0))
                                                .text_color(palette.rose),
                                        )
                                        .tooltip("删除此插件配置")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.delete_loaded_plugin(path_for_del.clone(), cx);
                                        })),
                                )
                            })
                    })),
            )
        })
}
