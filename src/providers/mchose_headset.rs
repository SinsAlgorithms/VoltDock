use std::collections::BTreeMap;

use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

const VID_V9_PRO: u16 = 0x291D;
const PID_V9_PRO: u16 = 0x385D;

const VID_MCHOSE_3837: u16 = 0x3837;
const PID_X9: u16 = 0x6045;
const PID_V9_TURBO_DONGLE: u16 = 0x6008;
const PID_V9_TURBO_BODY: u16 = 0x600A;

const REPORT_ID_STATUS: u8 = 0x55;
const MSG_TYPE_STATUS: u8 = 0x65;
const CMD_QUERY_BATTERY: u8 = 0x01;

pub struct MchoseHeadsetProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadsetParsed {
    pub level: Option<u8>,
    pub state: PowerState,
    pub raw_status: u8,
}

pub fn build_v9_pro_request() -> [u8; 64] {
    let mut buf = [0u8; 64];
    buf[0] = REPORT_ID_STATUS;
    buf[1] = MSG_TYPE_STATUS;
    buf[2] = CMD_QUERY_BATTERY;
    buf
}

pub fn build_turbo_aa_request() -> [u8; 64] {
    let mut buf = [0u8; 64];
    buf[0] = REPORT_ID_STATUS;
    buf[1] = 0xAA;
    buf[2] = 0x01; // version
    buf[3] = 0x00; // flags
    buf[4] = 0x01; // length (payload + 1)
    buf[5] = 0x00; // cmdType hi
    buf[6] = 0x01; // cmdType lo (Request)
    buf[7] = 0x0B; // cmd battery
    buf[8] = 0x00; // checksum slot
    buf
}

pub fn parse_65_reply(resp: &[u8], is_v9_pro_291d: bool) -> Option<HeadsetParsed> {
    if resp.len() < 4 || resp[0] != REPORT_ID_STATUS || resp[1] != MSG_TYPE_STATUS {
        return None;
    }
    let level = resp[2];
    let status_byte = resp[3];

    if is_v9_pro_291d {
        if level > 100 {
            return None;
        }
        // Measured on 291D:385D: 0x00 = charging on cable, 0x02 = discharging on wireless
        let state = if status_byte == 0x00 {
            if level == 100 {
                PowerState::Full
            } else {
                PowerState::Charging
            }
        } else {
            PowerState::Discharging
        };
        return Some(HeadsetParsed {
            level: Some(level),
            state,
            raw_status: status_byte,
        });
    }

    // 0x3837 headsets (X9 / V9 Turbo): status_byte 2=discharging, 3=charging, 4=full, 26=asleep
    match status_byte {
        26 => Some(HeadsetParsed {
            level: None,
            state: PowerState::Sleeping,
            raw_status: status_byte,
        }),
        2..=4 if level <= 100 => {
            let state = match status_byte {
                3 => PowerState::Charging,
                4 => PowerState::Full,
                _ => PowerState::Discharging,
            };
            Some(HeadsetParsed {
                level: Some(level),
                state,
                raw_status: status_byte,
            })
        }
        _ => None,
    }
}

pub fn audio_checksum_ok(frame: &[u8]) -> bool {
    if frame.len() < 8 {
        return false;
    }
    let flags = frame[2];
    let length = frame[3] as usize;
    if length < 1 || frame.len() < 7 + length {
        return false;
    }
    let mut acc = flags ^ frame[3] ^ frame[4] ^ frame[5] ^ frame[6];
    for &b in &frame[7..7 + length - 1] {
        acc ^= b;
    }
    let expected = if flags == 1 { acc } else { 0 };
    frame[7 + length - 1] == expected
}

pub fn parse_turbo_aa_reply(resp: &[u8]) -> Option<HeadsetParsed> {
    if resp.len() < 11 || resp[0] != REPORT_ID_STATUS || resp[1] != 0xAA {
        return None;
    }
    if resp[5] != 0x00 || resp[6] != 0x01 || resp[7] != 0x0B {
        return None;
    }
    if !audio_checksum_ok(&resp[1..]) {
        return None;
    }
    let length = resp[4] as usize;
    if length < 3 || resp.len() < 8 + length - 1 {
        return None;
    }
    let payload = &resp[8..8 + length - 1];
    if payload[0] == 0 {
        return Some(HeadsetParsed {
            level: None,
            state: PowerState::Sleeping,
            raw_status: 0,
        });
    }
    let percent = payload[1] & 0x7F;
    if percent > 100 {
        return None;
    }
    let charging = (payload[1] >> 7) == 1;
    Some(HeadsetParsed {
        level: Some(percent),
        state: if charging {
            PowerState::Charging
        } else {
            PowerState::Discharging
        },
        raw_status: payload[1],
    })
}

fn is_supported_headset(vid: u16, pid: u16) -> bool {
    (vid == VID_V9_PRO && pid == PID_V9_PRO)
        || (vid == VID_MCHOSE_3837
            && matches!(pid, PID_X9 | PID_V9_TURBO_DONGLE | PID_V9_TURBO_BODY))
}

fn send_and_poll_64(
    handle: &HidHandle,
    req: &[u8; 64],
    in_len: usize,
    is_v9_pro_291d: bool,
    allow_aa: bool,
) -> anyhow::Result<Option<HeadsetParsed>> {
    let mut parsed_out = None;
    let try_queued = handle.write_and_read_queued(req, in_len.max(64), 500, 3, |pkt| {
        if allow_aa && let Some(p) = parse_turbo_aa_reply(pkt) {
            parsed_out = Some(p);
            return true;
        }
        if let Some(p) = parse_65_reply(pkt, is_v9_pro_291d) {
            parsed_out = Some(p);
            return true;
        }
        false
    });

    match try_queued {
        Ok(_) => Ok(parsed_out),
        Err(err) => {
            // Some Windows drivers reject WriteFile and require Feature or SetOutputReport
            let msg = err.to_string();
            if msg.contains("err=1") || msg.contains("incorrect function") {
                handle.set_feature(req)?;
                for _ in 0..3 {
                    if let Some(pkt) = handle.read_overlapped(in_len.max(64), 250)? {
                        if allow_aa && let Some(p) = parse_turbo_aa_reply(&pkt) {
                            return Ok(Some(p));
                        }
                        if let Some(p) = parse_65_reply(&pkt, is_v9_pro_291d) {
                            return Ok(Some(p));
                        }
                    }
                }
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

fn query_headset_collection(
    info: &HidDeviceInfo,
    is_v9_pro_291d: bool,
    is_turbo: bool,
    diag: &mut Vec<String>,
) -> Option<HeadsetParsed> {
    let handle = match HidHandle::open(&info.path, true) {
        Ok(h) => h,
        Err(e) => {
            diag.push(format!(
                "[mchose-headset] open {:04X}:{:04X} UP={:04X} err: {e}",
                info.vendor_id, info.product_id, info.usage_page
            ));
            return None;
        }
    };

    if is_turbo {
        let aa_req = build_turbo_aa_request();
        if let Ok(Some(p)) = send_and_poll_64(&handle, &aa_req, info.input_report_len as usize, false, true) {
            diag.push(format!(
                "[mchose-headset] {:04X}:{:04X} AA reply -> level={:?} state={:?}",
                info.vendor_id, info.product_id, p.level, p.state
            ));
            return Some(p);
        }
    }

    let req = build_v9_pro_request();
    match send_and_poll_64(
        &handle,
        &req,
        info.input_report_len as usize,
        is_v9_pro_291d,
        false,
    ) {
        Ok(Some(p)) => {
            diag.push(format!(
                "[mchose-headset] {:04X}:{:04X} UP={:04X} 55-65 reply -> level={:?} state={:?} raw_status=0x{:02X}",
                info.vendor_id, info.product_id, info.usage_page, p.level, p.state, p.raw_status
            ));
            Some(p)
        }
        Ok(None) => {
            diag.push(format!(
                "[mchose-headset] {:04X}:{:04X} UP={:04X} timeout (headset off or asleep)",
                info.vendor_id, info.product_id, info.usage_page
            ));
            None
        }
        Err(e) => {
            diag.push(format!(
                "[mchose-headset] {:04X}:{:04X} UP={:04X} err: {e}",
                info.vendor_id, info.product_id, info.usage_page
            ));
            None
        }
    }
}

impl BatteryProvider for MchoseHeadsetProvider {
    fn name(&self) -> &'static str {
        "mchose_headset"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut grouped: BTreeMap<String, Vec<&HidDeviceInfo>> = BTreeMap::new();
        for d in ctx.hid_devices {
            if is_supported_headset(d.vendor_id, d.product_id) {
                let key = if d.vendor_id == VID_V9_PRO {
                    "mchose:v9_pro".to_string()
                } else if d.product_id == PID_X9 {
                    "mchose:x9".to_string()
                } else {
                    "mchose:v9_turbo".to_string()
                };
                grouped.entry(key).or_default().push(d);
            }
        }

        let mut readings = Vec::new();
        for (key, devs) in grouped {
            let vid = devs[0].vendor_id;
            let pid = devs[0].product_id;
            let is_v9_pro_291d = vid == VID_V9_PRO && pid == PID_V9_PRO;
            let is_turbo = key == "mchose:v9_turbo";

            let product = devs
                .iter()
                .map(|d| d.product.trim())
                .filter(|p| !p.is_empty())
                .max_by_key(|p| (p.contains('+'), p.len()))
                .unwrap_or(if is_v9_pro_291d {
                    "MCHOSE V9 PRO"
                } else if key == "mchose:x9" {
                    "MCHOSE X9"
                } else {
                    "MCHOSE V9 Turbo"
                })
                .to_string();

            // Strictly touch vendor collections >= 0xFF00 that carry 64-byte IN/OUT reports.
            // For 291D:385D and 3837:6045, prefer 0xFF90 exclusively when present.
            let mut cols: Vec<&HidDeviceInfo> = devs
                .iter()
                .copied()
                .filter(|d| {
                    d.usage_page >= 0xFF00
                        && d.input_report_len >= 64
                        && d.output_report_len >= 64
                })
                .collect();

            if !is_turbo && cols.iter().any(|d| d.usage_page == 0xFF90) {
                cols.retain(|d| d.usage_page == 0xFF90);
            }

            cols.sort_by_key(|d| {
                let pref = if is_turbo {
                    if d.usage_page == 0xFF00 { 0 } else { 1 }
                } else if d.usage_page == 0xFF90 {
                    0
                } else {
                    1
                };
                (pref, d.usage_page)
            });

            if cols.is_empty() {
                diag.push(format!(
                    "[mchose-headset] {vid:04X}:{pid:04X} '{product}' no 64B vendor collection"
                ));
                continue;
            }

            let mut parsed: Option<HeadsetParsed> = None;
            for col in cols {
                if let Some(res) = query_headset_collection(col, is_v9_pro_291d, is_turbo, diag) {
                    parsed = Some(res);
                    break;
                }
            }

            if let Some(p) = parsed {
                readings.push(DeviceReading {
                    id: key,
                    name: product,
                    brand: Some("MCHOSE".to_string()),
                    kind: DeviceKind::Headset,
                    connection: if p.state == PowerState::Charging {
                        ConnectionType::Wired
                    } else {
                        ConnectionType::Dongle24G
                    },
                    battery_percent: p.level,
                    power_state: p.state,
                    provider: self.name().to_string(),
                    vid_pid: Some((vid, pid)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: if p.state == PowerState::Sleeping {
                        Some("耳机未开机或已休眠".to_string())
                    } else {
                        None
                    },
                });
            } else {
                readings.push(DeviceReading {
                    id: key,
                    name: product,
                    brand: Some("MCHOSE".to_string()),
                    kind: DeviceKind::Headset,
                    connection: ConnectionType::Dongle24G,
                    battery_percent: None,
                    power_state: PowerState::Offline,
                    provider: self.name().to_string(),
                    vid_pid: Some((vid, pid)),
                    mac_address: None,
                    last_seen: Local::now(),
                    approximate: false,
                    detail: Some("接收器在线，耳机未开机/未连接".to_string()),
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
    fn parse_v9_pro_replies() {
        // 50% discharging (status 0x02)
        let r1 = [0x55, 0x65, 0x32, 0x02, 0x00];
        assert_eq!(
            parse_65_reply(&r1, true),
            Some(HeadsetParsed {
                level: Some(50),
                state: PowerState::Discharging,
                raw_status: 0x02,
            })
        );

        // 60% charging (status 0x00)
        let r2 = [0x55, 0x65, 0x3C, 0x00, 0x00];
        assert_eq!(
            parse_65_reply(&r2, true),
            Some(HeadsetParsed {
                level: Some(60),
                state: PowerState::Charging,
                raw_status: 0x00,
            })
        );

        // Out of range rejected
        let bad = [0x55, 0x65, 0x65, 0x02, 0x00];
        assert!(parse_65_reply(&bad, true).is_none());
    }

    #[test]
    fn parse_x9_and_turbo_aa_replies() {
        // X9 asleep (status 26 = 0x1A)
        let x9_sleep = [0x55, 0x65, 0x00, 26];
        assert_eq!(
            parse_65_reply(&x9_sleep, false),
            Some(HeadsetParsed {
                level: None,
                state: PowerState::Sleeping,
                raw_status: 26,
            })
        );

        // AA-framed response: flags=0 (checksum 0), length=3, cmdType=00 01, cmd=0B, payload=[sleep=1, 0x80|85]
        let mut aa = [0u8; 64];
        aa[0] = 0x55;
        aa[1] = 0xAA;
        aa[2] = 0x01;
        aa[3] = 0x00;
        aa[4] = 0x03;
        aa[5] = 0x00;
        aa[6] = 0x01;
        aa[7] = 0x0B;
        aa[8] = 0x01; // awake
        aa[9] = 0x80 | 85; // charging at 85%
        aa[10] = 0x00; // checksum=0 when flags==0
        assert_eq!(
            parse_turbo_aa_reply(&aa),
            Some(HeadsetParsed {
                level: Some(85),
                state: PowerState::Charging,
                raw_status: 0x80 | 85,
            })
        );
    }
}
