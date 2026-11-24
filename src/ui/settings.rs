use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{AppConfig, LanguagePreference, ThemePreference};
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
    let lang = config.language;
    let intervals: &[(u64, &str)] = if lang.is_en() {
        &[(10, "10s"), (30, "30s"), (60, "1m"), (120, "2m")]
    } else {
        &[(10, "10秒"), (30, "30秒"), (60, "1分"), (120, "2分")]
    };
    let themes: &[(ThemePreference, &str)] = &[
        (ThemePreference::Dark, lang.pick("暗色", "Dark")),
        (ThemePreference::Light, lang.pick("亮色", "Light")),
    ];
    let languages: &[(LanguagePreference, &str)] = &[
        (LanguagePreference::Zh, "中文"),
        (LanguagePreference::En, "English"),
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
                        .child(lang.pick("刷新频率", "Refresh Interval")),
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
                        .child(lang.pick("精简每行设备数", "Compact Columns")),
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
                        .child(lang.pick("外观主题", "Theme")),
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
        // Language row
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.text_secondary)
                        .child(lang.pick("界面语言", "Language")),
                )
                .child(h_flex().gap_1().children(languages.iter().enumerate().map(|(i, &(pref, label))| {
                    let active = config.language == pref;
                    div()
                        .id(("lang", i))
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_language(pref, cx);
                        }))
                        .child(label)
                }))),
        )
        // Switch rows
        .child(toggle_row(
            "sw-top",
            lang.pick("始终置顶", "Always on top"),
            config.always_on_top,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_always_on_top(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-sleep",
            lang.pick("显示最近休眠的设备", "Show recently sleeping"),
            config.show_sleeping,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_show_sleeping(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-phone",
            lang.pick("显示蓝牙手机", "Show Bluetooth phones"),
            config.show_phones,
            palette,
            cx.listener(|this, _, window, cx| {
                this.toggle_show_phones(window, cx);
            }),
        ))
        .child(toggle_row(
            "sw-notify",
            lang.pick("低电量弹窗提醒 (<=20%)", "Low battery alert (<=20%)"),
            config.low_battery_notify,
            palette,
            cx.listener(|this, _, _, cx| {
                this.toggle_low_battery_notify(cx);
            }),
        ))
        .child(toggle_row(
            "sw-autostart",
            lang.pick("开机自启动", "Launch at startup"),
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
                                .label(lang.pick("扫描/插件", "Scanner"))
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
                                .label(lang.pick("插件目录", "Plugins Dir"))
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
                            lang.pick("收起日志", "Hide Logs")
                        } else {
                            lang.pick("诊断", "Logs")
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
