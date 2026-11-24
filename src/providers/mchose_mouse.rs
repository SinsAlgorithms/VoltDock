use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

const VID_REALTEK: u16 = 0x5253;
const VID_MCHOSE_NEW: u16 = 0x3837;
const VID_G7_YJX: u16 = 0xA8A5;
const PID_G7: u16 = 0x2255;
const VID_A5_GEN1: u16 = 0x2023;

const CONFIG_USAGE_PAGE: u16 = 0xFF01;
const A5_USAGE_PAGE: u16 = 0xFFFF;

const SHORT_REPORT_ID: u8 = 0x11;
const LONG_REPORT_ID: u8 = 0x12;
const SHORT_PAYLOAD_LEN: usize = 20;
const LONG_PAYLOAD_LEN: usize = 64;
const CMD_STATUS: u8 = 0x06;

// Headset PIDs under VID 0x3837 are handled by mchose_headset.
const MCHOSE_3837_HEADSET_PIDS: &[u16] = &[0x6008, 0x600A, 0x6045];

pub struct MchoseMouseProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MchoseMouseStatus {
    pub vid: u16,
    pub model: u16,
    pub flags: u8,
    pub level: u8,
    pub charging: bool,
}

pub fn build_inverted_request(report_id: u8, cmd: u8, payload_len: usize) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + payload_len);
    frame.push(report_id);
    for i in 0..payload_len {
        let raw = if i == 0 { cmd } else { 0x00 };
        frame.push(raw ^ 0xFF);
    }
    frame
}

pub fn parse_inverted_status(resp: &[u8]) -> Option<MchoseMouseStatus> {
    if resp.len() < 13 {
        return None;
    }
    if resp[0] != SHORT_REPORT_ID && resp[0] != LONG_REPORT_ID {
        return None;
    }
    if resp[1] != (CMD_STATUS ^ 0xFF) {
        return None;
    }
    let pay: Vec<u8> = resp[2..].iter().map(|b| b ^ 0xFF).collect();
    if pay.len() < 11 {
        return None;
    }
    let vid = u16::from_le_bytes([pay[0], pay[1]]);
    if vid != VID_REALTEK && vid != VID_MCHOSE_NEW {
        return None;
    }
    let model = u16::from_le_bytes([pay[2], pay[3]]);
    let flags = pay[8];
    let level = pay[9];
    let charging = pay[10] != 0;
    if level > 100 {
        return None;
    }
    Some(MchoseMouseStatus {
        vid,
        model,
        flags,
        level,
        charging,
    })
}

pub fn build_g7_request(out_len: usize) -> Vec<u8> {
    let len = out_len.max(65);
    let mut buf = vec![0u8; len];
    let hdr = [0x00, 0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01];
    buf[..hdr.len()].copy_from_slice(&hdr);
    buf
}

pub fn parse_g7_status(resp: &[u8]) -> Option<(u8, bool)> {
    if resp.len() < 10 || resp[0] != 0xAA || resp[1] != 0x30 {
        return None;
    }
    let level = resp[8];
    if level > 100 {
        return None;
    }
    Some((level, resp[9] != 0))
}

pub fn build_a5_request() -> Vec<u8> {
    let mut frame = vec![0u8; 65];
    frame[0] = 0x00;
    frame[1] = 0x00;
    frame[2] = 0x00;
    frame[3] = 0x02; // route = mouse
    frame[4] = 0x02; // len
    frame[5] = 0x00; // page
    frame[6] = 0x83; // cmd battery
    frame
}

pub fn parse_a5_status(resp: &[u8]) -> Option<(u8, bool)> {
    if resp.len() < 9 {
        return None;
    }
    let at = if resp[0] == 0xA1 {
        0
    } else if resp[1] == 0xA1 {
        1
    } else {
        return None;
    };
    if resp.len() < at + 8 {
        return None;
    }
    if resp[at + 4] != 0x00 || resp[at + 5] != 0x83 {
        return None;
    }
    let charging = resp[at + 6] > 0;
    let level = resp[at + 7].min(100);
    Some((level, charging))
}

fn a5_pid_name(pid: u16) -> Option<&'static str> {
    match pid {
        0xF013 => Some("MCHOSE A5 Pro Max (1K)"),
        0xF015 => Some("MCHOSE A5 Pro Max (4K)"),
        0xF019 => Some("MCHOSE A5 Pro Max (有线)"),
        _ => None,
    }
}

fn resolve_mouse_name(product: &str, model: Option<u16>) -> String {
    let trimmed = product.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("USB Input Device") {
        return trimmed.to_string();
    }
    match model {
        Some(0x0031) => "MCHOSE M7 Ultra".to_string(),
        Some(0x0011) => "MCHOSE A7 Ultra".to_string(),
        Some(m) => format!("MCHOSE 鼠标 (0x{m:04X})"),
        None => "MCHOSE 无线鼠标".to_string(),
    }
}

fn is_mchose_mouse_vid_pid(vid: u16, pid: u16) -> bool {
    match vid {
        VID_REALTEK => true,
        VID_MCHOSE_NEW => !MCHOSE_3837_HEADSET_PIDS.contains(&pid),
        VID_G7_YJX => pid == PID_G7,
        VID_A5_GEN1 => a5_pid_name(pid).is_some(),
        _ => false,
    }
}

fn query_realtek_or_3837(
    info: &HidDeviceInfo,
    diag: &mut Vec<String>,
) -> Option<MchoseMouseStatus> {
    let handle = match HidHandle::open(&info.path, false) {
        Ok(h) => h,
        Err(e) => {
            diag.push(format!(
                "[mchose-mouse] open {:04X}:{:04X} failed: {e}",
                info.vendor_id, info.product_id
            ));
            return None;
        }
    };

    for (rid, plen) in [
        (SHORT_REPORT_ID, SHORT_PAYLOAD_LEN),
        (LONG_REPORT_ID, LONG_PAYLOAD_LEN),
    ] {
        let req = build_inverted_request(rid, CMD_STATUS, plen);
        let read_len = (1 + plen).max(info.feature_report_len as usize);
        for attempt in 0..4 {
            if let Err(e) = handle.set_feature(&req) {
                diag.push(format!(
                    "[mchose-mouse] set_feature rid=0x{rid:02X} att={attempt} err: {e}"
                ));
                break;
            }
            thread::sleep(Duration::from_millis(60));
            match handle.get_feature(rid, read_len) {
                Ok(resp) => {
                    if let Some(parsed) = parse_inverted_status(&resp) {
                        diag.push(format!(
                            "[mchose-mouse] {:04X}:{:04X} rid=0x{rid:02X} att={attempt} -> {}% charging={} model=0x{:04X} flags=0x{:02X}",
                            info.vendor_id,
                            info.product_id,
                            parsed.level,
                            parsed.charging,
                            parsed.model,
                            parsed.flags
                        ));
                        return Some(parsed);
                    }
                }
                Err(e) => {
                    diag.push(format!(
                        "[mchose-mouse] get_feature rid=0x{rid:02X} att={attempt} err: {e}"
                    ));
                    break;
                }
            }
            thread::sleep(Duration::from_millis(60));
        }
    }
    None
}

fn query_g7(info: &HidDeviceInfo, diag: &mut Vec<String>) -> Option<(u8, bool)> {
    let handle = match HidHandle::open(&info.path, true) {
        Ok(h) => h,
        Err(e) => {
            diag.push(format!("[mchose-g7] open failed: {e}"));
            return None;
        }
    };
    let req = build_g7_request(info.output_report_len as usize);
    let in_len = (info.input_report_len as usize).max(65);
    match handle.write_and_read_queued(&req, in_len, 600, 12, |pkt| parse_g7_status(pkt).is_some())
    {
        Ok(Some(pkt)) => {
            let res = parse_g7_status(&pkt);
            if let Some((lvl, chg)) = res {
                diag.push(format!("[mchose-g7] -> {lvl}% charging={chg}"));
            }
            res
        }
        Ok(None) => {
            diag.push("[mchose-g7] no AA 30 reply (asleep)".to_string());
            None
        }
        Err(e) => {
            diag.push(format!("[mchose-g7] I/O err: {e}"));
            None
        }
    }
}

fn query_a5(info: &HidDeviceInfo, diag: &mut Vec<String>) -> Option<(u8, bool)> {
    let handle = match HidHandle::open(&info.path, false) {
        Ok(h) => h,
        Err(e) => {
            diag.push(format!("[mchose-a5] open failed: {e}"));
            return None;
        }
    };
    let req = build_a5_request();
    let read_len = (info.feature_report_len as usize).max(65);
    if let Err(e) = handle.set_feature(&req) {
        diag.push(format!("[mchose-a5] set_feature err: {e}"));
        return None;
    }
    thread::sleep(Duration::from_millis(34));
    for attempt in 0..8 {
        if attempt == 3 {
            let _ = handle.set_feature(&req);
            thread::sleep(Duration::from_millis(34));
        }
        if let Ok(resp) = handle.get_feature(0x00, read_len)
            && let Some(parsed) = parse_a5_status(&resp)
        {
            diag.push(format!(
                "[mchose-a5] -> {}% charging={}",
                parsed.0, parsed.1
            ));
            return Some(parsed);
        }
        thread::sleep(Duration::from_millis(30));
    }
    diag.push("[mchose-a5] no 0xA1 reply (asleep)".to_string());
    None
}

impl BatteryProvider for MchoseMouseProvider {
    fn name(&self) -> &'static str {
        "mchose_mouse"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut grouped: BTreeMap<(u16, u16), Vec<&HidDeviceInfo>> = BTreeMap::new();
        for d in ctx.hid_devices {
            if is_mchose_mouse_vid_pid(d.vendor_id, d.product_id) {
                grouped
                    .entry((d.vendor_id, d.product_id))
                    .or_default()
                    .push(d);
            }
        }

        let mut readings = Vec::new();
        for ((vid, pid), devs) in grouped {
            let product = devs
                .iter()
                .map(|d| d.product.trim())
                .find(|p| !p.is_empty())
                .unwrap_or("")
                .to_string();

            if vid == VID_G7_YJX {
                let Some(col) = devs.iter().find(|d| d.usage_page == CONFIG_USAGE_PAGE) else {
                    continue;
                };
                if let Some((lvl, chg)) = query_g7(col, diag) {
                    readings.push(DeviceReading {
                        id: "mchose:g7".to_string(),
                        name: resolve_mouse_name(&product, None),
                        brand: Some("MCHOSE".to_string()),
                        kind: DeviceKind::Mouse,
                        connection: if chg {
                            ConnectionType::Wired
                        } else {
                            ConnectionType::Dongle24G
                        },
                        battery_percent: Some(lvl),
                        power_state: if chg {
                            PowerState::Charging
                        } else {
                            PowerState::Discharging
                        },
                        provider: self.name().to_string(),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: None,
                    });
                } else {
                    readings.push(DeviceReading {
                        id: "mchose:g7".to_string(),
                        name: resolve_mouse_name(&product, None),
                        brand: Some("MCHOSE".to_string()),
                        kind: DeviceKind::Mouse,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: None,
                        power_state: PowerState::Offline,
                        provider: self.name().to_string(),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: Some("接收器在线，鼠标未连接/休眠".to_string()),
                    });
                }
                continue;
            }

            if vid == VID_A5_GEN1 {
                let Some(col) = devs.iter().find(|d| d.usage_page == A5_USAGE_PAGE) else {
                    continue;
                };
                let default_name = a5_pid_name(pid).unwrap_or("MCHOSE A5 Pro Max");
                let name = if product.is_empty() {
                    default_name.to_string()
                } else {
                    product.clone()
                };
                if let Some((lvl, chg)) = query_a5(col, diag) {
                    readings.push(DeviceReading {
                        id: "mchose:a5_gen1".to_string(),
                        name,
                        brand: Some("MCHOSE".to_string()),
                        kind: DeviceKind::Mouse,
                        connection: if pid == 0xF019 || chg {
                            ConnectionType::Wired
                        } else {
                            ConnectionType::Dongle24G
                        },
                        battery_percent: Some(lvl),
                        power_state: if chg {
                            PowerState::Charging
                        } else {
                            PowerState::Discharging
                        },
                        provider: self.name().to_string(),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: None,
                    });
                } else {
                    readings.push(DeviceReading {
                        id: "mchose:a5_gen1".to_string(),
                        name,
                        brand: Some("MCHOSE".to_string()),
                        kind: DeviceKind::Mouse,
                        connection: ConnectionType::Dongle24G,
                        battery_percent: None,
                        power_state: PowerState::Offline,
                        provider: self.name().to_string(),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: Some("接收器在线，鼠标未连接/休眠".to_string()),
                    });
                }
                continue;
            }

            // 0x5253 or 0x3837 mice
            let mut cols: Vec<&HidDeviceInfo> = devs
                .iter()
                .copied()
                .filter(|d| d.usage_page == CONFIG_USAGE_PAGE)
                .collect();
            cols.sort_by_key(|d| d.usage != 1);

            if cols.is_empty() {
                diag.push(format!(
                    "[mchose-mouse] {vid:04X}:{pid:04X} '{product}' has no 0xFF01 collection"
                ));
                continue;
            }

            let mut answered = None;
            for col in &cols {
                if let Some(st) = query_realtek_or_3837(col, diag) {
                    answered = Some(st);
                    break;
                }
            }

            let dev_key = if vid == VID_REALTEK {
                "mchose:mouse_5253".to_string()
            } else {
                format!("mchose:mouse_{vid:04x}_{pid:04x}")
            };

            if let Some(st) = answered {
                let conn = if st.flags == 0x00 && st.charging {
                    ConnectionType::Wired
                } else {
                    ConnectionType::Dongle24G
                };
                readings.push(DeviceReading {
                    id: dev_key,
                    name: resolve_mouse_name(&product, Some(st.model)),
                    brand: Some("MCHOSE".to_string()),
                    kind: DeviceKind::Mouse,
                    connection: conn,
                    battery_percent: Some(st.level),
                    power_state: if st.charging {
                        if st.level == 100 {
                            PowerState::Full
                        } else {
                            PowerState::Charging
                        }
                    } else {
                        PowerState::Discharging
                    },
                    provider: self.name().to_string(),
                    vid_pid: Some((vid, pid)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: Some(format!("型号 0x{:04X}", st.model)),
                });
            } else {
                diag.push(format!(
                    "[mchose-mouse] {vid:04X}:{pid:04X} '{product}' silent -> offline"
                ));
                readings.push(DeviceReading {
                    id: dev_key,
                    name: resolve_mouse_name(&product, None),
                    brand: Some("MCHOSE".to_string()),
                    kind: DeviceKind::Mouse,
                    connection: ConnectionType::Dongle24G,
                    battery_percent: None,
                    power_state: PowerState::Offline,
                    provider: self.name().to_string(),
                    vid_pid: Some((vid, pid)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: Some("接收器在线，鼠标未连接/休眠".to_string()),
                });
            }
        }
        readings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtek_mchose_a7_captured_frame() {
        // Real frame captured live from user's MCHOSE A7 Ultra (5253:1020)
        // hdr=11f9acad dec[:16]=5352110005000704094f005100000000
        let decoded_payload = [
            0x53, 0x52, 0x11, 0x00, 0x05, 0x00, 0x07, 0x04, 0x09, 0x4f, 0x00, 0x51, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut resp = vec![0x11, 0x06 ^ 0xFF];
        resp.extend(decoded_payload.iter().map(|b| b ^ 0xFF));

        let st = parse_inverted_status(&resp).expect("should parse live A7 Ultra frame");
        assert_eq!(st.vid, 0x5253);
        assert_eq!(st.model, 0x0011);
        assert_eq!(st.flags, 0x09);
        assert_eq!(st.level, 79);
        assert!(!st.charging);
    }

    #[test]
    fn realtek_rejects_all_zero_sleeping_frame() {
        // When sleeping, inverted payload is all 0xFF -> decoded all 0x00 -> vid == 0x0000
        let mut resp = vec![0x11, 0x06 ^ 0xFF];
        resp.extend(std::iter::repeat_n(0xFFu8, 19));
        assert!(parse_inverted_status(&resp).is_none());
    }

    #[test]
    fn g7_and_a5_parsers() {
        let g7 = [0xAA, 0x30, 0xA5, 0x3C, 0x0A, 0x01, 0x01, 0x01, 0x2E, 0x01];
        assert_eq!(parse_g7_status(&g7), Some((46, true)));

        let a5 = [0xA1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x01, 25, 0x00];
        assert_eq!(parse_a5_status(&a5), Some((25, true)));
    }
}
