use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{AppConfig, ThemePreference};
use crate::platform_win::open_folder_in_explorer;
use crate::ui::Palette;
use crate::ui::widget::BatteryWidget;

pub fn render_settings_panel(
    config: &AppConfig,
    show_diagnostics: bool,
    diagnostics: &[String],
    palette: &Palette,
    cx: &mut Context<BatteryWidget>,
) -> impl IntoElement {
    let intervals: &[(u64, &str)] = &[
        (10, "10秒"),
        (30, "30秒"),
        (60, "1分"),
        (120, "2分"),
    ];
    let themes: &[(ThemePreference, &str)] = &[
        (ThemePreference::Dark, "暗色"),
        (ThemePreference::Light, "亮色"),
    ];
    let per_row_choices: &[usize] = &[1, 2, 3, 4, 5];

    v_flex()
        .w_full()
        .px_2p5()
        .py_2()
        .gap_2()
        .border_t_1()
        .border_color(palette.divider)
        // Poll interval row
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.text_secondary)
                        .child("刷新频率"),
                )
                .child(h_flex().gap_1().children(intervals.iter().map(|&(secs, label)| {
                    let active = config.poll_interval_secs == secs;
                    div()
                        .id(("intv", secs as usize))
                        .cursor_pointer()
                        .px_1p5()
                        .py(px(2.0))
                        .rounded(px(5.0))
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_poll_interval(secs, cx);
                        }))
                        .child(label)
                }))),
        )
        // Compact max-per-row selector
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.text_secondary)
                        .child("精简每行设备数"),
                )
                .child(h_flex().gap_1().children(per_row_choices.iter().map(|&n| {
                    let active = config.compact_max_per_row == n;
                    div()
                        .id(("per-row", n))
                        .cursor_pointer()
                        .px_2()
                        .py(px(2.0))
                        .rounded(px(5.0))
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
                            this.set_compact_max_per_row(n, window, cx);
                        }))
                        .child(format!("{n}"))
                }))),
        )
        // Theme row
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.text_secondary)
                        .child("外观主题"),
                )
                .child(h_flex().gap_1().children(themes.iter().enumerate().map(|(i, &(pref, label))| {
                    let active = config.theme_mode == pref
                        || (config.theme_mode == ThemePreference::System && pref == ThemePreference::Dark);
                    div()
                        .id(("theme", i))
                        .cursor_pointer()
                        .px_2()
                        .py(px(2.0))
                        .rounded(px(5.0))
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
                            this.set_theme_mode(pref, window, cx);
                        }))
                        .child(label)
                }))),
        )
        // Switch rows
        .child(toggle_row(
            "sw-top",
            "始终置顶",
            config.always_on_top,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_always_on_top(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-sleep",
            "显示最近休眠的设备",
            config.show_sleeping,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_show_sleeping(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-phone",
            "显示蓝牙手机",
            config.show_phones,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_show_phones(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-notify",
            "低电量弹窗提醒 (<=20%)",
            config.low_battery_notify,
            palette,
            cx.listener(|this, _, _, cx| {
                this.toggle_low_battery_notify(cx);
            }),
        ))
        .child(toggle_row(
            "sw-autostart",
            "开机自启动",
            config.auto_start,
            palette,
            cx.listener(|this, _, _, cx| {
                this.toggle_auto_start(cx);
            }),
        ))
        // Plugin & Diagnostics quick bar
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .pt_1()
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("btn-open-scanner")
                                .secondary()
                                .xsmall()
                                .icon(
                                    Icon::default()
                                        .path("icons/scan.svg")
                                        .size(px(12.0))
                                        .text_color(palette.accent),
                                )
                                .label("扫描/插件")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_scanner(window, cx);
                                })),
                        )
                        .child(
                            Button::new("btn-open-plugins-dir")
                                .ghost()
                                .xsmall()
                                .icon(
                                    Icon::default()
                                        .path("icons/folder.svg")
                                        .size(px(12.0))
                                        .text_color(palette.text_secondary),
                                )
                                .label("插件目录")
                                .on_click(|_, _, _| {
                                    open_folder_in_explorer(&AppConfig::plugins_dir());
                                }),
                        ),
                )
                .child(
                    Button::new("btn-diag")
                        .ghost()
                        .xsmall()
                        .icon(
                            Icon::default()
                                .path("icons/bug.svg")
                                .size(px(12.0))
                                .text_color(palette.text_secondary),
                        )
                        .label(if show_diagnostics {
                            "收起日志"
                        } else {
                            "诊断"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_diagnostics(window, cx);
                        })),
                ),
        )
        .when(show_diagnostics, |this| {
            let log_lines: Vec<String> = diagnostics.iter().take(28).cloned().collect();
            this.child(
                v_flex()
                    .id("diag-box")
                    .w_full()
                    .h(px(116.0))
                    .p_2()
                    .gap(px(2.0))
                    .rounded(px(6.0))
                    .bg(palette.card_bg)
                    .border_1()
                    .border_color(palette.card_border)
                    .overflow_y_scroll()
                    .children(log_lines.into_iter().enumerate().map(|(i, line)| {
                        div()
                            .id(("diag-line", i))
                            .text_size(px(10.0))
                            .text_color(palette.text_secondary)
                            .child(line)
                    })),
            )
        })
}

fn toggle_row(
    id: &'static str,
    label: &'static str,
    checked: bool,
    palette: &Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let rc_handler = std::rc::Rc::new(handler);
    let h_label = rc_handler.clone();
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(
            div()
                .id((id, 1u32))
                .flex_1()
                .cursor_pointer()
                .text_xs()
                .text_color(palette.text_secondary)
                .on_click(move |ev, window, cx| {
                    (h_label)(ev, window, cx);
                })
                .child(label),
        )
        .child(
            Switch::new(id)
                .small()
                .checked(checked)
                .on_change(move |_, window, cx| {
                    let dummy = ClickEvent::default();
                    (rc_handler)(&dummy, window, cx);
                }),
        )
}
