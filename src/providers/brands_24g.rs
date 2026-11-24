use std::collections::BTreeMap;

use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

const VID_LOGITECH: u16 = 0x046D;
const VID_RAZER: u16 = 0x1532;
const VID_STEELSERIES: u16 = 0x1038;
const VID_HYPERX_HP: u16 = 0x03F0;
const VID_HYPERX_KINGSTON: u16 = 0x0951;
const VID_CORSAIR: u16 = 0x1B1C;

pub struct Brands24GProvider;

// ============================================================================
// 1. Logitech HID++ 2.0 (Lightspeed / Unifying / Bolt / Wired / Headsets)
// ============================================================================

const HIDPP_LONG_REPORT: u8 = 0x11;
const HIDPP_SWID: u8 = 0x0A;

pub fn build_hidpp_request(slot: u8, feat_idx: u8, func: u8, params: &[u8]) -> [u8; 20] {
    let mut pkt = [0u8; 20];
    pkt[0] = HIDPP_LONG_REPORT;
    pkt[1] = slot;
    pkt[2] = feat_idx;
    pkt[3] = ((func & 0x0F) << 4) | (HIDPP_SWID & 0x0F);
    let copy_len = params.len().min(16);
    pkt[4..4 + copy_len].copy_from_slice(&params[..copy_len]);
    pkt
}

pub fn hidpp_voltage_to_percent(mv: u16) -> u8 {
    // Standard 1-cell LiPo discharge curve (4.20V -> 3.50V)
    const CURVE: &[(u16, u8)] = &[
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3807, 50),
        (3761, 40),
        (3722, 30),
        (3682, 20),
        (3620, 10),
        (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    if mv <= CURVE[CURVE.len() - 1].0 {
        return 0;
    }
    for w in CURVE.windows(2) {
        let (v_hi, p_hi) = w[0];
        let (v_lo, p_lo) = w[1];
        if mv <= v_hi && mv >= v_lo {
            let num = (mv - v_lo) as u32 * (p_hi - p_lo) as u32;
            let den = (v_hi - v_lo) as u32;
            return (p_lo as u32 + num / den).min(100) as u8;
        }
    }
    50
}

pub fn parse_hidpp_1004_or_1000(params: &[u8]) -> Option<(u8, PowerState)> {
    if params.len() < 3 {
        return None;
    }
    let pct = params[0];
    if pct > 100 {
        return None;
    }
    let state = match params[2] {
        1 | 4 => PowerState::Charging,
        2 | 3 => PowerState::Full,
        _ => PowerState::Discharging,
    };
    Some((pct, state))
}

fn hidpp_exchange(
    handle: &HidHandle,
    slot: u8,
    feat_idx: u8,
    func: u8,
    params: &[u8],
) -> Option<Vec<u8>> {
    let req = build_hidpp_request(slot, feat_idx, func, params);
    let expected_sub = ((func & 0x0F) << 4) | (HIDPP_SWID & 0x0F);
    handle
        .write_and_read_queued(&req, 20, 260, 4, |pkt| {
            if pkt.len() < 7 || pkt[1] != slot {
                return false;
            }
            if matches!(pkt[0], 0x10 | 0x11) && (pkt[2] == 0x8F || pkt[2] == 0xFF) {
                return true; // HID++ error reply
            }
            pkt[0] == HIDPP_LONG_REPORT && pkt[2] == feat_idx && pkt[3] == expected_sub
        })
        .ok()
        .flatten()
}

fn is_hidpp_error(pkt: &[u8]) -> bool {
    pkt.len() >= 6 && (pkt[2] == 0x8F || pkt[2] == 0xFF)
}

fn hidpp_find_feature(handle: &HidHandle, slot: u8, feature_id: u16) -> Option<u8> {
    let hi = (feature_id >> 8) as u8;
    let lo = (feature_id & 0xFF) as u8;
    let resp = hidpp_exchange(handle, slot, 0x00, 0x00, &[hi, lo])?;
    if is_hidpp_error(&resp) || resp.len() < 5 || resp[4] == 0 {
        return None;
    }
    Some(resp[4])
}

fn poll_logitech(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    let mut seen_paths = Vec::new();

    for d in hid_devices {
        if d.vendor_id != VID_LOGITECH {
            continue;
        }
        let is_long_hidpp = (d.usage_page == 0xFF00 && d.usage == 0x0002)
            || (d.usage_page == 0xFF43 && d.usage == 0x0202)
            || (d.usage_page == 0x000C && d.product_id == 0x0AA8); // G535
        if !is_long_hidpp || d.output_report_len < 20 {
            continue;
        }
        if seen_paths.contains(&d.path) {
            continue;
        }
        seen_paths.push(d.path.clone());

        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };

        let is_receiver = d.usage_page == 0xFF00
            && (d.product.to_ascii_lowercase().contains("receiver")
                || d.product.to_ascii_lowercase().contains("lightspeed")
                || d.product.to_ascii_lowercase().contains("bolt")
                || d.product.to_ascii_lowercase().contains("unifying"));

        let slots: &[u8] = if is_receiver { &[1, 2, 3, 4, 5, 6] } else { &[0xFF] };

        for &slot in slots {
            // Ping via Root feature 0x0000 fn 1
            let Some(ping) = hidpp_exchange(&handle, slot, 0x00, 0x01, &[0, 0, 0x5A]) else {
                continue;
            };
            if is_hidpp_error(&ping) {
                continue;
            }

            let mut battery: Option<(u8, PowerState)> = None;
            // Try 0x1004 (Unified Battery) -> 0x1000 (Battery Status) -> 0x1001 (Voltage) -> 0x1F20 (ADC)
            if let Some(idx) = hidpp_find_feature(&handle, slot, 0x1004)
                && let Some(resp) = hidpp_exchange(&handle, slot, idx, 0x01, &[])
                && !is_hidpp_error(&resp)
            {
                battery = parse_hidpp_1004_or_1000(&resp[4..]);
            }
            if battery.is_none()
                && let Some(idx) = hidpp_find_feature(&handle, slot, 0x1000)
                && let Some(resp) = hidpp_exchange(&handle, slot, idx, 0x00, &[])
                && !is_hidpp_error(&resp)
            {
                battery = parse_hidpp_1004_or_1000(&resp[4..]);
            }
            if battery.is_none()
                && let Some(idx) = hidpp_find_feature(&handle, slot, 0x1001)
                && let Some(resp) = hidpp_exchange(&handle, slot, idx, 0x00, &[])
                && !is_hidpp_error(&resp)
                && resp.len() >= 7
            {
                let mv = u16::from_be_bytes([resp[4], resp[5]]);
                let chg = (resp[6] & 0x80) != 0;
                let pct = hidpp_voltage_to_percent(mv);
                battery = Some((
                    pct,
                    if chg {
                        PowerState::Charging
                    } else {
                        PowerState::Discharging
                    },
                ));
            }
            if battery.is_none()
                && let Some(idx) = hidpp_find_feature(&handle, slot, 0x1F20)
                && let Some(resp) = hidpp_exchange(&handle, slot, idx, 0x00, &[])
                && !is_hidpp_error(&resp)
                && resp.len() >= 7
            {
                let mv = u16::from_be_bytes([resp[4], resp[5]]);
                let chg = (resp[6] & 0x80) != 0;
                battery = Some((
                    hidpp_voltage_to_percent(mv),
                    if chg {
                        PowerState::Charging
                    } else {
                        PowerState::Discharging
                    },
                ));
            }

            let Some((pct, st)) = battery else {
                continue;
            };

            let mut dev_name = d.product.trim().to_string();
            // Try reading real device name via feature 0x0005 (GetDeviceNameType)
            if let Some(name_idx) = hidpp_find_feature(&handle, slot, 0x0005)
                && let Some(len_resp) = hidpp_exchange(&handle, slot, name_idx, 0x00, &[])
                && !is_hidpp_error(&len_resp)
                && len_resp.len() >= 5
            {
                let total_len = (len_resp[4] as usize).min(48);
                let mut raw_name = Vec::new();
                while raw_name.len() < total_len {
                    if let Some(chunk) =
                        hidpp_exchange(&handle, slot, name_idx, 0x01, &[raw_name.len() as u8])
                        && !is_hidpp_error(&chunk)
                        && chunk.len() > 4
                    {
                        raw_name.extend_from_slice(&chunk[4..]);
                    } else {
                        break;
                    }
                }
                raw_name.truncate(total_len);
                let s = String::from_utf8_lossy(&raw_name).trim_matches('\0').trim().to_string();
                if !s.is_empty() {
                    dev_name = s;
                }
            }
            if dev_name.is_empty() {
                dev_name = "Logitech 无线设备".to_string();
            }

            let lower = dev_name.to_ascii_lowercase();
            let kind = if d.usage_page == 0xFF43
                || lower.contains("headset")
                || lower.contains("g733")
                || lower.contains("g535")
                || lower.contains("g pro x")
            {
                DeviceKind::Headset
            } else if lower.contains("keyboard") || lower.starts_with("g915") || lower.contains("mx keys") {
                DeviceKind::Keyboard
            } else {
                DeviceKind::Mouse
            };

            diag.push(format!(
                "[logitech] {:04X} slot={slot} '{dev_name}' -> {pct}% st={st:?}",
                d.product_id
            ));
            out.push(DeviceReading {
                id: format!("logi:{:04x}:slot{slot}", d.product_id),
                name: dev_name,
                brand: Some("Logitech".to_string()),
                kind,
                connection: if slot == 0xFF && st == PowerState::Charging {
                    ConnectionType::Wired
                } else {
                    ConnectionType::Dongle24G
                },
                battery_percent: Some(pct),
                power_state: st,
                provider: "logitech_hidpp".to_string(),
                vid_pid: Some((VID_LOGITECH, d.product_id)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail: None,
            });
        }
    }
    out
}

// ============================================================================
// 2. Razer Wireless 90-byte Feature Report Protocol
// ============================================================================

pub fn build_razer_feature(tx_id: u8, cmd_class: u8, cmd_id: u8) -> [u8; 91] {
    // Index 0 is Report ID 0x00 for HidD_SetFeature; bytes 1..91 are the 90-byte Razer report
    let mut buf = [0u8; 91];
    buf[1] = 0x00; // status = new command
    buf[2] = tx_id;
    buf[6] = 0x02; // data size
    buf[7] = cmd_class;
    buf[8] = cmd_id;
    let mut crc = 0u8;
    for &b in &buf[3..89] {
        crc ^= b;
    }
    buf[89] = crc;
    buf
}

pub fn parse_razer_battery(resp: &[u8]) -> Option<u8> {
    // HidD_GetFeature returns 91 bytes with report ID 0x00 at index 0 -> payload starts at 1
    let off = if resp.len() >= 91 && resp[0] == 0x00 {
        1
    } else if resp.len() >= 90 {
        0
    } else {
        return None;
    };
    let status = resp[off];
    if status != 0x02 {
        return None;
    }
    let raw = resp[off + 9]; // args[1]
    Some(((raw as f64 / 255.0) * 100.0).round().clamp(0.0, 100.0) as u8)
}

fn poll_razer(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut grouped: BTreeMap<u16, Vec<&HidDeviceInfo>> = BTreeMap::new();
    for d in hid_devices {
        if d.vendor_id == VID_RAZER && d.feature_report_len >= 90 {
            grouped.entry(d.product_id).or_default().push(d);
        }
    }
    let mut out = Vec::new();
    for (pid, cols) in grouped {
        for col in cols {
            let Ok(handle) = HidHandle::open(&col.path, false) else {
                continue;
            };
            let mut got_pct = None;
            let mut used_tx = 0x1F;
            for tx in [0x1F, 0x3F, 0xFF] {
                let req = build_razer_feature(tx, 0x07, 0x80);
                if handle.set_feature(&req).is_ok() {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    if let Ok(resp) = handle.get_feature(0x00, 91)
                        && let Some(pct) = parse_razer_battery(&resp)
                    {
                        got_pct = Some(pct);
                        used_tx = tx;
                        break;
                    }
                }
            }
            if let Some(pct) = got_pct {
                let mut charging = false;
                let req_chg = build_razer_feature(used_tx, 0x07, 0x84);
                if handle.set_feature(&req_chg).is_ok() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    if let Ok(resp) = handle.get_feature(0x00, 91) {
                        let off = if resp.len() >= 91 && resp[0] == 0x00 { 1 } else { 0 };
                        if resp[off] == 0x02 {
                            charging = resp[off + 9] != 0;
                        }
                    }
                }
                let name = if col.product.trim().is_empty() {
                    format!("Razer 无线设备 (0x{pid:04X})")
                } else {
                    col.product.trim().to_string()
                };
                let lower = name.to_ascii_lowercase();
                let kind = if lower.contains("blackshark")
                    || lower.contains("barracuda")
                    || lower.contains("kraken")
                    || lower.contains("nari")
                {
                    DeviceKind::Headset
                } else if lower.contains("keyboard") || lower.contains("blackwidow") || lower.contains("deathstalker") {
                    DeviceKind::Keyboard
                } else {
                    DeviceKind::Mouse
                };
                diag.push(format!("[razer] 1532:{pid:04X} '{name}' -> {pct}% charging={charging}"));
                out.push(DeviceReading {
                    id: format!("razer:{pid:04x}"),
                    name,
                    brand: Some("Razer".to_string()),
                    kind,
                    connection: ConnectionType::Dongle24G,
                    battery_percent: Some(pct),
                    power_state: if charging {
                        PowerState::Charging
                    } else {
                        PowerState::Discharging
                    },
                    provider: "razer_hid".to_string(),
                    vid_pid: Some((VID_RAZER, pid)),
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

// ============================================================================
// 3. SteelSeries, HyperX, Corsair, CompX ODM Mice (VGN / ATK / Ajazz / Lamzu)
// ============================================================================

pub fn parse_steelseries_b0(resp: &[u8]) -> Option<(u8, PowerState)> {
    let off = if resp.len() >= 5 && resp[0] == 0xB0 {
        0
    } else if resp.len() >= 6 && resp[1] == 0xB0 {
        1
    } else {
        return None;
    };
    let b1 = resp[off + 1];
    let b2 = resp[off + 2];
    let b3 = resp[off + 3];
    // Nova 7 style: b1 = link (03 connected, 02 disconnected), b2 = level, b3 = status
    if b1 == 0x03 && b2 <= 100 && matches!(b3, 1..=3) {
        let state = if b3 == 1 || b3 == 2 {
            PowerState::Charging
        } else {
            PowerState::Discharging
        };
        return Some((b2, state));
    }
    // Nova 5 / Nova 3P style: b3 = level (0..=100), b4 = charging
    if resp.len() >= off + 5 && b1 != 0x02 && b3 <= 100 {
        let chg = resp[off + 4] == 1;
        return Some((
            b3,
            if chg {
                PowerState::Charging
            } else {
                PowerState::Discharging
            },
        ));
    }
    None
}

pub fn build_compx_power_request() -> [u8; 17] {
    let mut pkt = [0u8; 17];
    pkt[0] = 0x08;
    pkt[1] = 0x04; // CMD_POWER
    let sum: u8 = pkt[..16].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    pkt[16] = 0x55u8.wrapping_sub(sum);
    pkt
}

pub fn parse_compx_power_reply(resp: &[u8]) -> Option<(u8, bool)> {
    if resp.len() < 10 || resp[0] != 0x08 || resp[1] != 0x04 {
        return None;
    }
    let level = resp[6];
    if level > 100 {
        return None;
    }
    Some((level, resp[7] != 0))
}

fn poll_steelseries(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_STEELSERIES || d.usage_page != 0xFFC0 {
            continue;
        }
        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };
        let mut req = vec![0u8; (d.output_report_len as usize).max(64)];
        req[0] = 0x00;
        req[1] = 0xB0;
        if let Ok(Some(pkt)) = handle.write_and_read_queued(
            &req,
            (d.input_report_len as usize).max(64),
            400,
            4,
            |p| parse_steelseries_b0(p).is_some(),
        ) && let Some((pct, st)) = parse_steelseries_b0(&pkt)
        {
            let name = if d.product.trim().is_empty() {
                "SteelSeries 无线耳机".to_string()
            } else {
                d.product.trim().to_string()
            };
            diag.push(format!("[steelseries] {:04X} '{name}' -> {pct}%", d.product_id));
            out.push(DeviceReading {
                id: format!("steelseries:{:04x}", d.product_id),
                name,
                brand: Some("SteelSeries".to_string()),
                kind: DeviceKind::Headset,
                connection: ConnectionType::Dongle24G,
                battery_percent: Some(pct),
                power_state: st,
                provider: "steelseries_hid".to_string(),
                vid_pid: Some((VID_STEELSERIES, d.product_id)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail: None,
            });
        }
    }
    out
}

fn poll_hyperx(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if !matches!(d.vendor_id, VID_HYPERX_HP | VID_HYPERX_KINGSTON) || d.usage_page != 0xFF13 {
            continue;
        }
        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };
        let mut req = [0u8; 62];
        req[0] = 0x66;
        req[1] = 0x89;
        let pkt_opt = handle
            .write_and_read_queued(&req, (d.input_report_len as usize).max(62), 350, 3, |p| {
                p.len() >= 5 && p[0] == 0x66 && matches!(p[1], 0x89 | 0x0D) && (p[2] != 0 || p[3] != 0)
            })
            .ok()
            .flatten();
        if let Some(pkt) = pkt_opt
            && pkt[4] <= 100
        {
            let pct = pkt[4];
            let mut chg_req = [0u8; 62];
            chg_req[0] = 0x66;
            chg_req[1] = 0x8A;
            let charging = handle
                .write_and_read_queued(
                    &chg_req,
                    (d.input_report_len as usize).max(62),
                    300,
                    3,
                    |p| p.len() >= 3 && p[0] == 0x66 && matches!(p[1], 0x8A | 0x0C),
                )
                .ok()
                .flatten()
                .is_some_and(|p| p[2] == 1 || p[2] == 2);
            let name = if d.product.trim().is_empty() {
                "HyperX 无线耳机".to_string()
            } else {
                d.product.trim().to_string()
            };
            diag.push(format!("[hyperx] {:04X} '{name}' -> {pct}%", d.product_id));
            out.push(DeviceReading {
                id: format!("hyperx:{:04x}", d.product_id),
                name,
                brand: Some("HyperX".to_string()),
                kind: DeviceKind::Headset,
                connection: ConnectionType::Dongle24G,
                battery_percent: Some(pct),
                power_state: if charging {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                },
                provider: "hyperx_hid".to_string(),
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

fn poll_corsair(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        if d.vendor_id != VID_CORSAIR {
            continue;
        }
        if d.usage_page == 0xFFC5 && d.output_report_len >= 2 {
            // Void Elite Wireless
            let Ok(handle) = HidHandle::open(&d.path, true) else {
                continue;
            };
            let req = [0xC9u8, 0x64];
            if let Ok(Some(pkt)) = handle.write_and_read_queued(&req, 5, 350, 3, |p| {
                p.len() >= 5 && p[0] == 0x64 && p[3] == 0xB1
            }) {
                let status = pkt[4];
                if status != 0 {
                    let pct = (pkt[2] & 0x7F).min(100);
                    let chg = status == 4 || status == 5;
                    let name = if d.product.trim().is_empty() {
                        "Corsair 无线耳机".to_string()
                    } else {
                        d.product.trim().to_string()
                    };
                    diag.push(format!("[corsair] Void '{name}' -> {pct}%"));
                    out.push(DeviceReading {
                        id: format!("corsair:{:04x}", d.product_id),
                        name,
                        brand: Some("Corsair".to_string()),
                        kind: DeviceKind::Headset,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: Some(pct),
                        power_state: if chg {
                            PowerState::Charging
                        } else {
                            PowerState::Discharging
                        },
                        provider: "corsair_hid".to_string(),
                        vid_pid: Some((VID_CORSAIR, d.product_id)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: None,
                    });
                }
            }
        } else if d.usage_page == 0xFF42 && d.output_report_len >= 64 {
            // Slipstream / HS80 / Virtuoso
            let Ok(handle) = HidHandle::open(&d.path, true) else {
                continue;
            };
            let mut req = [0u8; 64];
            req[0] = 0x02;
            req[1] = 0x09; // headset behind receiver
            req[2] = 0x02;
            req[3] = 0x0F; // battery tenths of %
            if let Ok(Some(pkt)) = handle.write_and_read_queued(&req, 64, 350, 4, |p| {
                p.len() >= 6 && p[0] == 0x01 && p[2] == 0x02 && p[3] == 0x00
            }) {
                let tenths = u16::from_le_bytes([pkt[4], pkt[5]]);
                if tenths <= 1000 {
                    let pct = ((tenths + 5) / 10).min(100) as u8;
                    let name = if d.product.trim().is_empty() {
                        "Corsair 无线耳机".to_string()
                    } else {
                        d.product.trim().to_string()
                    };
                    diag.push(format!("[corsair] Slipstream '{name}' -> {pct}%"));
                    out.push(DeviceReading {
                        id: format!("corsair:{:04x}", d.product_id),
                        name,
                        brand: Some("Corsair".to_string()),
                        kind: DeviceKind::Headset,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: Some(pct),
                        power_state: PowerState::Discharging,
                        provider: "corsair_hid".to_string(),
                        vid_pid: Some((VID_CORSAIR, d.product_id)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: None,
                    });
                }
            }
        }
    }
    out
}

fn poll_compx_mice(hid_devices: &[HidDeviceInfo], diag: &mut Vec<String>) -> Vec<DeviceReading> {
    let mut out = Vec::new();
    for d in hid_devices {
        // CompX ODM 2.4GHz mice (VGN, ATK, Darmoshark, Ajazz, Lamzu, Incott, etc.)
        // Note: exclude MCHOSE / GameSir / Holtek keyboard already handled elsewhere
        if d.usage_page != 0xFF02 || d.usage != 0x0002 || d.output_report_len < 17 {
            continue;
        }
        if matches!(d.vendor_id, 0x5253 | 0x3837 | 0x3537 | 0x260D) {
            continue;
        }
        let Ok(handle) = HidHandle::open(&d.path, true) else {
            continue;
        };
        let req = build_compx_power_request();
        let in_len = (d.input_report_len as usize).max(17);
        if let Ok(Some(pkt)) =
            handle.write_and_read_queued(&req, in_len, 300, 4, |p| parse_compx_power_reply(p).is_some())
            && let Some((pct, chg)) = parse_compx_power_reply(&pkt)
        {
            let name = if d.product.trim().is_empty() {
                format!("CompX 无线鼠标 ({:04X}:{:04X})", d.vendor_id, d.product_id)
            } else {
                d.product.trim().to_string()
            };
            diag.push(format!("[compx] {:04X}:{:04X} '{name}' -> {pct}% chg={chg}", d.vendor_id, d.product_id));
            out.push(DeviceReading {
                id: format!("compx:{:04x}:{:04x}", d.vendor_id, d.product_id),
                name,
                brand: if d.manufacturer.trim().is_empty() {
                    None
                } else {
                    Some(d.manufacturer.trim().to_string())
                },
                kind: DeviceKind::Mouse,
                connection: if chg {
                    ConnectionType::Wired
                } else {
                    ConnectionType::Dongle24G
                },
                battery_percent: Some(pct),
                power_state: if chg {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                },
                provider: "compx_hid".to_string(),
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
fn poll_generic_hid_battery(
    hid_devices: &[HidDeviceInfo],
    diag: &mut Vec<String>,
) -> Vec<DeviceReading> {
    use windows::Win32::Devices::HumanInterfaceDevice::{
        HIDP_VALUE_CAPS, HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_Feature,
        HidP_GetUsageValue, HidP_GetValueCaps, PHIDP_PREPARSED_DATA,
    };
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::core::PCWSTR;

    let mut out = Vec::new();
    for d in hid_devices {
        // Usage Page 0x0084 (Power Device) or 0x0085 (Battery System)
        if !matches!(d.usage_page, 0x0084 | 0x0085) || d.feature_report_len == 0 {
            continue;
        }
        let mut wide: Vec<u16> = d.path.encode_utf16().collect();
        wide.push(0);
        let raw_h = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        let Ok(h): Result<HANDLE, _> = raw_h else {
            continue;
        };
        let mut ppd = PHIDP_PREPARSED_DATA::default();
        let mut found_cap = None;
        unsafe {
            if HidD_GetPreparsedData(h, &mut ppd) {
                let mut count = 32u16;
                let mut caps = vec![HIDP_VALUE_CAPS::default(); count as usize];
                if HidP_GetValueCaps(HidP_Feature, caps.as_mut_ptr(), &mut count, ppd).0 >= 0 {
                    caps.truncate(count as usize);
                    // Battery System (0x85) RemainingCapacity (0x66) or RelativeStateOfCharge (0x65)
                    for c in &caps {
                        if c.UsagePage == 0x0085 && matches!(c.Anonymous.NotRange.Usage, 0x0065 | 0x0066) {
                            found_cap = Some((c.ReportID, c.UsagePage, c.Anonymous.NotRange.Usage, c.LogicalMax));
                            break;
                        }
                    }
                }
                if let Some((rid, up, u, lmax)) = found_cap
                    && let Ok(io_h) = HidHandle::open(&d.path, false)
                    && let Ok(report) = io_h.get_feature(rid, d.feature_report_len as usize)
                {
                    let mut val = 0u32;
                    if HidP_GetUsageValue(HidP_Feature, up, None, u, &mut val, ppd, &report).0 >= 0 {
                        let pct = if lmax > 0 && lmax != 100 {
                            ((val as f64 / lmax as f64) * 100.0).round().clamp(0.0, 100.0) as u8
                        } else {
                            (val as u8).min(100)
                        };
                        let name = if d.product.trim().is_empty() {
                            format!("HID 电池设备 ({:04X}:{:04X})", d.vendor_id, d.product_id)
                        } else {
                            d.product.trim().to_string()
                        };
                        diag.push(format!("[hid-battery] {:04X}:{:04X} '{name}' -> {pct}%", d.vendor_id, d.product_id));
                        out.push(DeviceReading {
                            id: format!("hidbat:{:04x}:{:04x}", d.vendor_id, d.product_id),
                            name,
                            brand: None,
                            kind: DeviceKind::Other,
                            connection: ConnectionType::Dongle24G,
                            battery_percent: Some(pct),
                            power_state: PowerState::Discharging,
                            provider: "hid_battery".to_string(),
                            vid_pid: Some((d.vendor_id, d.product_id)),
                            mac_address: None,
                            last_seen: Local::now(),
                            approximate: false,
                            detail: None,
                        });
                    }
                }
                let _ = HidD_FreePreparsedData(ppd);
            }
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn poll_generic_hid_battery(_: &[HidDeviceInfo], _: &mut Vec<String>) -> Vec<DeviceReading> {
    Vec::new()
}

impl BatteryProvider for Brands24GProvider {
    fn name(&self) -> &'static str {
        "brands_24g"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut out = Vec::new();
        out.extend(poll_logitech(ctx.hid_devices, diag));
        out.extend(poll_razer(ctx.hid_devices, diag));
        out.extend(poll_steelseries(ctx.hid_devices, diag));
        out.extend(poll_hyperx(ctx.hid_devices, diag));
        out.extend(poll_corsair(ctx.hid_devices, diag));
        out.extend(poll_compx_mice(ctx.hid_devices, diag));
        out.extend(poll_generic_hid_battery(ctx.hid_devices, diag));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidpp_voltage_and_unified_battery() {
        assert_eq!(hidpp_voltage_to_percent(4200), 100);
        assert_eq!(hidpp_voltage_to_percent(3500), 0);
        assert!(hidpp_voltage_to_percent(3807) == 50);
        assert_eq!(
            parse_hidpp_1004_or_1000(&[82, 0x04, 0x00]),
            Some((82, PowerState::Discharging))
        );
        assert_eq!(
            parse_hidpp_1004_or_1000(&[64, 0x02, 0x01]),
            Some((64, PowerState::Charging))
        );
    }

    #[test]
    fn razer_crc_and_compx_checksum() {
        let req = build_razer_feature(0x1F, 0x07, 0x80);
        assert_eq!(req[2], 0x1F);
        assert_eq!(req[7], 0x07);
        assert_eq!(req[8], 0x80);

        let mut resp = [0u8; 91];
        resp[1] = 0x02; // OK
        resp[10] = 0x80; // ~50% (128/255 = 50.19%)
        assert_eq!(parse_razer_battery(&resp), Some(50));

        let cx = build_compx_power_request();
        let sum: u8 = cx.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        assert_eq!(sum, 0x55);

        let cx_reply = [0x08, 0x04, 0, 0, 0, 0, 73, 1, 0, 0];
        assert_eq!(parse_compx_power_reply(&cx_reply), Some((73, true)));
    }
}
