use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

const VID_SONY: u16 = 0x054C;
const VID_NINTENDO: u16 = 0x057E;
const VID_8BITDO: u16 = 0x2DC8;
const VID_FLYDIGI: u16 = 0x37D7;

pub struct StandardGamepadProvider;

pub fn parse_ps4_report(pkt: &[u8]) -> Option<(u8, bool, bool)> {
    // Returns (percent, charging, is_bluetooth)
    if pkt.is_empty() {
        return None;
    }
    let (byte, is_bt) = if pkt[0] == 0x01 && pkt.len() >= 31 {
        (pkt[30], false)
    } else if pkt[0] == 0x11 && pkt.len() >= 33 {
        (pkt[32], true)
    } else {
        return None;
    };
    let cable = (byte & 0x10) != 0;
    let raw_lvl = byte & 0x0F;
    let pct = if cable {
        if raw_lvl > 10 {
            100
        } else {
            (raw_lvl * 10).min(100)
        }
    } else {
        ((raw_lvl.saturating_add(1)) * 10).min(100)
    };
    Some((pct, cable, is_bt))
}

pub fn parse_dualsense_report(pkt: &[u8]) -> Option<(u8, PowerState, bool)> {
    // Returns (percent, state, is_bluetooth)
    if pkt.is_empty() {
        return None;
    }
    let (status_byte, is_bt) = if pkt[0] == 0x01 && pkt.len() >= 54 {
        (pkt[53], false)
    } else if pkt[0] == 0x31 && pkt.len() >= 55 {
        (pkt[54], true)
    } else {
        return None;
    };
    let raw_lvl = status_byte & 0x0F;
    let charge_state = (status_byte >> 4) & 0x0F;
    let pct = (raw_lvl * 10 + 5).min(100);
    let state = match charge_state {
        0 => PowerState::Discharging,
        1 => PowerState::Charging,
        2 => return Some((100, PowerState::Full, is_bt)),
        _ => PowerState::Discharging,
    };
    Some((pct, state, is_bt))
}

pub fn parse_switch_report(pkt: &[u8]) -> Option<(u8, bool)> {
    if pkt.len() < 3 || !matches!(pkt[0], 0x21 | 0x30 | 0x31) {
        return None;
    }
    let bat_nibble = (pkt[2] >> 4) & 0x0F;
    let charging = (bat_nibble & 0x01) != 0;
    let level_steps = bat_nibble >> 1; // 0..=4 -> 0,25,50,75,100
    let pct = ((level_steps.min(4) as u16) * 25) as u8;
    Some((pct, charging))
}

pub fn parse_8bitdo_report(pkt: &[u8]) -> Option<(u8, bool)> {
    if pkt.len() < 15 || !matches!(pkt[0], 0x01 | 0x04) {
        return None;
    }
    let b = pkt[14];
    let level = b & 0x7F;
    if level == 0 || level > 100 {
        return None;
    }
    Some((level, (b & 0x80) != 0))
}

pub fn parse_flydigi_report(pkt: &[u8]) -> Option<(u8, PowerState)> {
    let off = if pkt.len() >= 15 && pkt[0] == 0x5A && pkt[1] == 0xA5 && pkt[2] == 0x01 {
        0
    } else if pkt.len() >= 16 && pkt[1] == 0x5A && pkt[2] == 0xA5 && pkt[3] == 0x01 {
        1
    } else {
        return None;
    };
    let b = pkt[off + 11];
    let state_nibble = (b >> 4) & 0x0F;
    let step = b & 0x0F;
    if state_nibble > 2 || step > 5 {
        return None;
    }
    let pct = (step * 20).min(100);
    let state = match state_nibble {
        1 => PowerState::Charging,
        2 => PowerState::Full,
        _ => PowerState::Discharging,
    };
    Some((pct, state))
}

pub fn build_flydigi_query() -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = 0x00;
    buf[1] = 0x5A;
    buf[2] = 0xA5;
    buf[3] = 0x01;
    buf[4] = 0x02;
    buf[5] = 0x00;
    buf
}

fn poll_sony_hid(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_SONY || d.usage_page != 0x0001 {
            continue;
        }
        let is_ds = matches!(d.product_id, 0x0CE6 | 0x0DF2);
        let is_ds4 = matches!(d.product_id, 0x05C4 | 0x09CC);
        if !is_ds && !is_ds4 {
            continue;
        }
        let Ok(handle) = HidHandle::open_read_only(&d.path, true) else {
            continue;
        };
        let len = (d.input_report_len as usize).max(64);
        for _ in 0..2 {
            let Ok(Some(pkt)) = handle.read_overlapped(len, 60) else {
                break;
            };
            if is_ds && let Some((pct, st, is_bt)) = parse_dualsense_report(&pkt) {
                let name = if d.product.trim().is_empty() {
                    if d.product_id == 0x0DF2 {
                        "DualSense Edge"
                    } else {
                        "DualSense 无线控制器"
                    }
                } else {
                    d.product.trim()
                };
                diag.push(format!("[gamepad] DualSense -> {pct}% bt={is_bt} st={st:?}"));
                out.push(DeviceReading {
                    id: format!("sony:{:04x}", d.product_id),
                    name: name.to_string(),
                    brand: Some("Sony".to_string()),
                    kind: DeviceKind::Gamepad,
                    connection: if is_bt {
                        ConnectionType::Bluetooth
                    } else {
                        ConnectionType::Wired
                    },
                    battery_percent: Some(pct),
                    power_state: st,
                    provider: "playstation_hid".to_string(),
                    vid_pid: Some((d.vendor_id, d.product_id)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: None,
                });
                break;
            }
            if is_ds4 && let Some((pct, chg, is_bt)) = parse_ps4_report(&pkt) {
                let name = if d.product.trim().is_empty() {
                    "DualShock 4 无线控制器"
                } else {
                    d.product.trim()
                };
                diag.push(format!("[gamepad] DualShock4 -> {pct}% bt={is_bt} chg={chg}"));
                out.push(DeviceReading {
                    id: format!("sony:{:04x}", d.product_id),
                    name: name.to_string(),
                    brand: Some("Sony".to_string()),
                    kind: DeviceKind::Gamepad,
                    connection: if is_bt {
                        ConnectionType::Bluetooth
                    } else {
                        ConnectionType::Wired
                    },
                    battery_percent: Some(pct),
                    power_state: if chg {
                        PowerState::Charging
                    } else {
                        PowerState::Discharging
                    },
                    provider: "playstation_hid".to_string(),
                    vid_pid: Some((d.vendor_id, d.product_id)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: None,
                });
                break;
            }
        }
    }
    out
}

fn poll_switch_hid(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_NINTENDO || !matches!(d.product_id, 0x2006 | 0x2007 | 0x2009) {
            continue;
        }
        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };
        let len = (d.input_report_len as usize).max(49);
        let mut parsed = None;
        if let Ok(Some(pkt)) = handle.read_overlapped(len, 45) {
            parsed = parse_switch_report(&pkt);
        }
        if parsed.is_none() {
            let mut req = [0u8; 49];
            req[0] = 0x01;
            req[2..10].copy_from_slice(&[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40]);
            req[10] = 0x02; // Request device info (read-only)
            if let Ok(Some(pkt)) = handle.write_and_read_queued(&req, len, 350, 4, |p| parse_switch_report(p).is_some()) {
                parsed = parse_switch_report(&pkt);
            }
        }
        if let Some((pct, chg)) = parsed {
            let name = match d.product_id {
                0x2006 => "Nintendo Joy-Con (L)",
                0x2007 => "Nintendo Joy-Con (R)",
                _ => "Nintendo Switch Pro",
            };
            diag.push(format!("[gamepad] Switch {:04X} -> {pct}% chg={chg}", d.product_id));
            out.push(DeviceReading {
                id: format!("nintendo:{:04x}", d.product_id),
                name: name.to_string(),
                brand: Some("Nintendo".to_string()),
                kind: DeviceKind::Gamepad,
                connection: ConnectionType::Bluetooth,
                battery_percent: Some(pct),
                power_state: if chg {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                },
                provider: "nintendo_hid".to_string(),
                vid_pid: Some((d.vendor_id, d.product_id)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail: None,
            });
        }
    }
    out
}

fn poll_8bitdo_hid(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_8BITDO || d.usage_page != 0x0001 {
            continue;
        }
        let Ok(handle) = HidHandle::open_read_only(&d.path, true) else {
            continue;
        };
        let len = (d.input_report_len as usize).max(16);
        if let Ok(Some(pkt)) = handle.read_overlapped(len, 50)
            && let Some((pct, chg)) = parse_8bitdo_report(&pkt)
        {
            let name = if d.product.trim().is_empty() {
                "8BitDo 无线手柄"
            } else {
                d.product.trim()
            };
            diag.push(format!("[gamepad] 8BitDo {:04X} -> {pct}% chg={chg}", d.product_id));
            out.push(DeviceReading {
                id: format!("8bitdo:{:04x}", d.product_id),
                name: name.to_string(),
                brand: Some("8BitDo".to_string()),
                kind: DeviceKind::Gamepad,
                connection: ConnectionType::Bluetooth,
                battery_percent: Some(pct),
                power_state: if chg {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                },
                provider: "8bitdo_hid".to_string(),
                vid_pid: Some((d.vendor_id, d.product_id)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail: None,
            });
        }
    }
    out
}

fn poll_flydigi_hid(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_FLYDIGI || d.usage_page != 0xFFA0 {
            continue;
        }
        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };
        let req = build_flydigi_query();
        let in_len = (d.input_report_len as usize).max(32);
        if let Ok(Some(pkt)) =
            handle.write_and_read_queued(&req, in_len, 450, 8, |p| parse_flydigi_report(p).is_some())
            && let Some((pct, st)) = parse_flydigi_report(&pkt)
        {
            let name = if d.product.trim().is_empty() {
                "飞智无线手柄"
            } else {
                d.product.trim()
            };
            diag.push(format!("[gamepad] FlyDigi {:04X} -> {pct}% st={st:?}", d.product_id));
            out.push(DeviceReading {
                id: format!("flydigi:{:04x}", d.product_id),
                name: name.to_string(),
                brand: Some("FlyDigi".to_string()),
                kind: DeviceKind::Gamepad,
                connection: ConnectionType::Dongle24G,
                battery_percent: Some(pct),
                power_state: st,
                provider: "flydigi_hid".to_string(),
                vid_pid: Some((d.vendor_id, d.product_id)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail: None,
            });
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn poll_wgi_and_xinput(diag: &mut Vec<String>) -> Vec<DeviceReading> {
    use windows::Gaming::Input::Gamepad;
    use windows::System::Power::BatteryStatus;
    use windows::Win32::UI::Input::XboxController::{
        BATTERY_DEVTYPE_GAMEPAD, BATTERY_LEVEL_EMPTY, BATTERY_LEVEL_FULL, BATTERY_LEVEL_LOW,
        BATTERY_LEVEL_MEDIUM, BATTERY_TYPE_ALKALINE, BATTERY_TYPE_DISCONNECTED,
        BATTERY_TYPE_NIMH, BATTERY_TYPE_WIRED, XINPUT_BATTERY_INFORMATION, XINPUT_STATE,
        XInputGetBatteryInformation, XInputGetState,
    };

    let mut readings = Vec::new();
    let mut wgi_reported = false;

    if let Ok(pads) = Gamepad::Gamepads()
        && let Ok(count) = pads.Size()
    {
        for idx in 0..count {
            let Ok(pad) = pads.GetAt(idx) else {
                continue;
            };
            let Ok(rep) = pad.TryGetBatteryReport() else {
                continue;
            };
            let Ok(status) = rep.Status() else {
                continue;
            };
            if status == BatteryStatus::NotPresent {
                continue;
            }
            let rem = rep.RemainingCapacityInMilliwattHours().ok().and_then(|r| r.Value().ok());
            let full = rep.FullChargeCapacityInMilliwattHours().ok().and_then(|r| r.Value().ok());
            if let (Some(r), Some(f)) = (rem, full)
                && f > 0
            {
                let pct = ((r as f64 / f as f64) * 100.0).round().clamp(0.0, 100.0) as u8;
                let pstate = match status {
                    BatteryStatus::Charging => PowerState::Charging,
                    BatteryStatus::Idle if pct == 100 => PowerState::Full,
                    _ => PowerState::Discharging,
                };
                diag.push(format!("[wgi] gamepad#{idx} -> {pct}% ({r}/{f} mWh) st={pstate:?}"));
                wgi_reported = true;
                readings.push(DeviceReading {
                    id: format!("wgi:gamepad:{idx}"),
                    name: if count == 1 {
                        "Xbox 无线手柄".to_string()
                    } else {
                        format!("Xbox 无线手柄 #{}", idx + 1)
                    },
                    brand: Some("Xbox".to_string()),
                    kind: DeviceKind::Gamepad,
                    connection: ConnectionType::Bluetooth,
                    battery_percent: Some(pct),
                    power_state: pstate,
                    provider: "wgi".to_string(),
                    vid_pid: None,
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: None,
                });
            }
        }
    }

    if !wgi_reported {
        for slot in 0..4u32 {
            let mut state = XINPUT_STATE::default();
            let rc = unsafe { XInputGetState(slot, &mut state) };
            if rc != 0 {
                continue;
            }
            let mut bat = XINPUT_BATTERY_INFORMATION::default();
            let brc = unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut bat) };
            if brc != 0 {
                continue;
            }
            if bat.BatteryType == BATTERY_TYPE_DISCONNECTED {
                continue;
            }
            if bat.BatteryType == BATTERY_TYPE_WIRED {
                // Wired or 2.4GHz dongle emulating wired XInput
                diag.push(format!("[xinput] slot#{slot} reports BATTERY_TYPE_WIRED"));
                readings.push(DeviceReading {
                    id: format!("xinput:{slot}"),
                    name: format!("XInput 手柄 #{}", slot + 1),
                    brand: None,
                    kind: DeviceKind::Gamepad,
                    connection: ConnectionType::Wired,
                    battery_percent: None,
                    power_state: PowerState::Charging,
                    provider: "xinput".to_string(),
                    vid_pid: None,
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: true,
                    detail: Some("有线或无电量上报接收器".to_string()),
                });
                continue;
            }
            if bat.BatteryType == BATTERY_TYPE_ALKALINE || bat.BatteryType == BATTERY_TYPE_NIMH {
                let pct = match bat.BatteryLevel {
                    BATTERY_LEVEL_EMPTY => 5,
                    BATTERY_LEVEL_LOW => 20,
                    BATTERY_LEVEL_MEDIUM => 55,
                    BATTERY_LEVEL_FULL => 100,
                    _ => continue,
                };
                diag.push(format!("[xinput] slot#{slot} -> ~{pct}%"));
                readings.push(DeviceReading {
                    id: format!("xinput:{slot}"),
                    name: format!("Xbox 手柄 #{}", slot + 1),
                    brand: Some("Xbox".to_string()),
                    kind: DeviceKind::Gamepad,
                    connection: ConnectionType::Dongle24G,
                    battery_percent: Some(pct),
                    power_state: if pct == 100 {
                        PowerState::Full
                    } else {
                        PowerState::Discharging
                    },
                    provider: "xinput".to_string(),
                    vid_pid: None,
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: true,
                    detail: Some("XInput 四段电量估算".to_string()),
                });
            }
        }
    }

    readings
}

#[cfg(not(target_os = "windows"))]
fn poll_wgi_and_xinput(_diag: &mut Vec<String>) -> Vec<DeviceReading> {
    Vec::new()
}

impl BatteryProvider for StandardGamepadProvider {
    fn name(&self) -> &'static str {
        "gamepad_std"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut out = Vec::new();
        out.extend(poll_sony_hid(ctx.hid_devices, diag));
        out.extend(poll_switch_hid(ctx.hid_devices, diag));
        out.extend(poll_8bitdo_hid(ctx.hid_devices, diag));
        out.extend(poll_flydigi_hid(ctx.hid_devices, diag));
        out.extend(poll_wgi_and_xinput(diag));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sony_switch_8bitdo_flydigi() {
        let mut ds = vec![0u8; 64];
        ds[0] = 0x31;
        ds[54] = 0x16; // charging, level 6 -> 65%
        assert_eq!(
            parse_dualsense_report(&ds),
            Some((65, PowerState::Charging, true))
        );

        let sw = [0x21, 0x00, 0x70]; // (0x70 >> 4)=7 -> charging bit1=1, step=3 -> 75%
        assert_eq!(parse_switch_report(&sw), Some((75, true)));

        let mut b8 = [0u8; 16];
        b8[0] = 0x04;
        b8[14] = 0x80 | 42;
        assert_eq!(parse_8bitdo_report(&b8), Some((42, true)));

        let mut fd = [0u8; 32];
        fd[0] = 0x5A;
        fd[1] = 0xA5;
        fd[2] = 0x01;
        fd[11] = 0x14; // charging (1), step 4 -> 80%
        assert_eq!(parse_flydigi_report(&fd), Some((80, PowerState::Charging)));
    }
}
