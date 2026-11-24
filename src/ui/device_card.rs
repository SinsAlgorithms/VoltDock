use gpui_kit::component::progress::Progress;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::LanguagePreference;
use crate::model::{DeviceReading, PowerState};
use crate::ui::Palette;

pub fn render_device_card(
    idx: usize,
    dev: &DeviceReading,
    display_name: String,
    low_threshold: u8,
    lang: LanguagePreference,
    palette: &Palette,
) -> impl IntoElement {
    let accent = palette.status_color(dev.battery_percent, dev.power_state);
    let inactive = dev.power_state.is_inactive();
    let is_charging = dev.power_state == PowerState::Charging;
    let is_low = dev.is_low_battery(low_threshold) && !inactive;

    let badge_bg = if inactive {
        palette.slate.opacity(0.14)
    } else {
        accent.opacity(0.16)
    };
    let icon_color = if inactive { palette.text_muted } else { accent };
    let name_color = if inactive {
        palette.text_secondary
    } else {
        palette.text_primary
    };
    let pct_color = if inactive {
        palette.text_muted
    } else if is_low {
        palette.rose
    } else {
        palette.text_primary
    };

    let mut status_parts = Vec::new();
    match dev.power_state {
        PowerState::Charging | PowerState::Full | PowerState::Sleeping | PowerState::Offline => {
            status_parts.push(dev.power_state.label(lang));
        }
        PowerState::Discharging | PowerState::Unknown => {
            if dev.approximate {
                status_parts.push(lang.pick("估算", "Est."));
            } else {
                status_parts.push(dev.kind.label(lang));
            }
        }
    }
    let status_text = status_parts.join(" · ");
    let bar_value = dev.battery_percent.unwrap_or(0) as f32;

    v_flex()
        .id(("dev-card", idx))
        .w_full()
        .px_2p5()
        .py_2()
        .gap_1p5()
        .rounded(px(9.0))
        .bg(palette.card_bg)
        .border_1()
        .border_color(if is_low {
            palette.rose.opacity(0.45)
        } else {
            palette.card_border
        })
        .hover(|s| s.bg(palette.card_hover_bg))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2p5()
                // Left device type icon badge
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .size(px(32.0))
                        .rounded(px(8.0))
                        .bg(badge_bg)
                        .child(
                            Icon::default()
                                .path(dev.kind.icon_path())
                                .size(px(17.0))
                                .text_color(icon_color),
                        ),
                )
                // Center name + connection/status row
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.0))
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(name_color)
                                .child(display_name),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                // Connection badge (2.4G / 蓝牙 / BLE / 有线)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap(px(3.0))
                                        .px_1p5()
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(palette.pill_bg)
                                        .child(
                                            Icon::default()
                                                .path(dev.connection.icon_path())
                                                .size(px(10.0))
                                                .text_color(palette.text_secondary),
                                        )
                                        .child(
                                            div()
                                                .text_color(palette.text_secondary)
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(dev.connection.badge_label_lang(lang)),
                                        ),
                                )
                                // Kind or state subtitle
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(if is_charging {
                                            palette.cyan
                                        } else {
                                            palette.text_muted
                                        })
                                        .child(status_text),
                                )
                                .when(is_low, |this| {
                                    this.child(
                                        div()
                                            .px_1()
                                            .py(px(0.5))
                                            .rounded(px(4.0))
                                            .bg(palette.rose.opacity(0.18))
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(palette.rose)
                                            .child(lang.pick("低电量", "Low")),
                                    )
                                }),
                        ),
                )
                // Right battery percentage + bolt
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .when(is_charging, |this| {
                            this.child(
                                Icon::default()
                                    .path("icons/bolt.svg")
                                    .size(px(13.0))
                                    .text_color(palette.cyan),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(pct_color)
                                .child(dev.display_percent()),
                        ),
                ),
        )
        // Bottom progress bar
        .child(
            Progress::new(("bat-prog", idx))
                .xsmall()
                .value(bar_value)
                .color(if inactive {
                    palette.slate.opacity(0.45)
                } else {
                    accent
                }),
        )
}

pub fn render_compact_chip(
    idx: usize,
    dev: &DeviceReading,
    display_name: String,
    lang: LanguagePreference,
    palette: &Palette,
) -> impl IntoElement {
    use gpui_kit::component::tooltip::Tooltip;

    let accent = palette.status_color(dev.battery_percent, dev.power_state);
    let inactive = dev.power_state.is_inactive();
    let is_charging = dev.power_state == PowerState::Charging;
    let tip: SharedString = format!(
        "{display_name} ({}) · {}",
        dev.connection.badge_label_lang(lang),
        dev.display_percent()
    )
    .into();

    h_flex()
        .id(("compact-chip", idx))
        .w(px(80.0))
        .h(px(28.0))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap_1p5()
        .px_2()
        .rounded(px(7.0))
        .bg(palette.card_bg)
        .border_1()
        .border_color(palette.card_border)
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(
            Icon::default()
                .path(dev.kind.icon_path())
                .size(px(13.5))
                .text_color(if inactive { palette.text_muted } else { accent }),
        )
        .when(is_charging, |this| {
            this.child(
                Icon::default()
                    .path("icons/bolt.svg")
                    .size(px(11.0))
                    .text_color(palette.cyan),
            )
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(if inactive {
                    palette.text_muted
                } else {
                    palette.text_primary
                })
                .child(dev.display_percent()),
        )
}
