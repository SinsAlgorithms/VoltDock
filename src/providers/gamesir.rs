use std::collections::BTreeMap;

use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

const VID_GAMESIR: u16 = 0x3537;
const USAGE_PAGE_GAMESIR: u16 = 0xFF7A;
const KNOWN_NOVA_PIDS: &[u16] = &[0x2106, 0x1098, 0x100F];

pub struct GameSirProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameSirBattery {
    Level(u8),
    Charging,
}

pub fn build_gamesir_query(out_len: usize) -> Vec<u8> {
    let mut req = vec![0u8; out_len.max(65)];
    req[0] = 0x00;
    req[1] = 0x01;
    req[2] = 0x01;
    req
}

pub fn is_gamesir_battery_frame(frame: &[u8]) -> bool {
    if frame.len() < 20 {
        return false;
    }
    if frame[1] == 0xF4 && frame[2] == 0x00 {
        return false;
    }
    frame[1] == 0x01
}

pub fn parse_gamesir_frame(frame: &[u8]) -> Option<GameSirBattery> {
    if !is_gamesir_battery_frame(frame) {
        return None;
    }
    let val = frame[19];
    if val == 0xFF {
        Some(GameSirBattery::Charging)
    } else if val <= 100 {
        Some(GameSirBattery::Level(val))
    } else {
        None
    }
}

pub fn friendly_gamesir_name(pid: u16, raw_product: &str) -> String {
    let t = raw_product.trim();
    let generic = t.is_empty()
        || t.eq_ignore_ascii_case("HID zkm")
        || t.eq_ignore_ascii_case("Zikway")
        || t.eq_ignore_ascii_case("USB Input Device");
    if !generic {
        return t.to_string();
    }
    if KNOWN_NOVA_PIDS.contains(&pid) {
        "GameSir Nova 2 Lite".to_string()
    } else {
        format!("GameSir 无线手柄 (0x{pid:04X})")
    }
}

fn is_candidate(d: &HidDeviceInfo) -> bool {
    if d.vendor_id != VID_GAMESIR {
        return false;
    }
    d.usage_page == USAGE_PAGE_GAMESIR
        || (KNOWN_NOVA_PIDS.contains(&d.product_id) && d.interface_number == Some(2))
}

impl BatteryProvider for GameSirProvider {
    fn name(&self) -> &'static str {
        "gamesir_hid"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut grouped: BTreeMap<u16, Vec<&HidDeviceInfo>> = BTreeMap::new();
        for d in ctx.hid_devices {
            if is_candidate(d) {
                grouped.entry(d.product_id).or_default().push(d);
            }
        }

        let mut readings = Vec::new();
        for (pid, mut cols) in grouped {
            cols.sort_by_key(|d| (d.usage_page != USAGE_PAGE_GAMESIR, d.output_report_len < 65));
            let raw_prod = cols
                .iter()
                .map(|d| d.product.trim())
                .find(|s| !s.is_empty())
                .unwrap_or("");
            let name = friendly_gamesir_name(pid, raw_prod);
            let dev_key = format!("gamesir:{pid:04x}");

            let mut reading_opt: Option<(Option<u8>, PowerState)> = None;
            for col in &cols {
                let handle = match HidHandle::open(&col.path, true) {
                    Ok(h) => h,
                    Err(e) => {
                        diag.push(format!(
                            "[gamesir] open 3537:{pid:04X} UP={:04X} err: {e}",
                            col.usage_page
                        ));
                        continue;
                    }
                };
                let req = build_gamesir_query(col.output_report_len as usize);
                let in_len = (col.input_report_len as usize).max(65);
                match handle.write_and_read_queued(&req, in_len, 1200, 12, is_gamesir_battery_frame)
                {
                    Ok(Some(pkt)) => match parse_gamesir_frame(&pkt) {
                        Some(GameSirBattery::Level(lvl)) => {
                            diag.push(format!(
                                "[gamesir] 3537:{pid:04X} UP={:04X} -> {lvl}%",
                                col.usage_page
                            ));
                            reading_opt = Some((Some(lvl), PowerState::Discharging));
                            break;
                        }
                        Some(GameSirBattery::Charging) => {
                            diag.push(format!(
                                "[gamesir] 3537:{pid:04X} UP={:04X} -> charging (0xFF)",
                                col.usage_page
                            ));
                            reading_opt = Some((None, PowerState::Charging));
                            break;
                        }
                        None => {
                            diag.push(format!(
                                "[gamesir] 3537:{pid:04X} unknown byte19=0x{:02X}",
                                pkt.get(19).copied().unwrap_or(0)
                            ));
                        }
                    },
                    Ok(None) => {
                        diag.push(format!(
                            "[gamesir] 3537:{pid:04X} UP={:04X} timeout (controller off/asleep)",
                            col.usage_page
                        ));
                    }
                    Err(e) => {
                        diag.push(format!(
                            "[gamesir] 3537:{pid:04X} UP={:04X} I/O err: {e}",
                            col.usage_page
                        ));
                    }
                }
            }

            let (battery_percent, power_state, detail) = match reading_opt {
                Some((lvl, st)) => (lvl, st, None),
                None => (
                    None,
                    PowerState::Offline,
                    Some("接收器在线，手柄未开机/未连接".to_string()),
                ),
            };

            readings.push(DeviceReading {
                id: dev_key,
                name,
                brand: Some("GameSir".to_string()),
                kind: DeviceKind::Gamepad,
                connection: ConnectionType::Dongle24G,
                battery_percent,
                power_state,
                provider: self.name().to_string(),
                vid_pid: Some((VID_GAMESIR, pid)),
                mac_address: None,
                last_seen: Local::now(),
                approximate: false,
                detail,
            });
        }
        readings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_and_charging_frames() {
        let mut frame = vec![0u8; 65];
        frame[1] = 0x01;
        frame[19] = 75;
        assert_eq!(parse_gamesir_frame(&frame), Some(GameSirBattery::Level(75)));

        frame[19] = 0xFF;
        assert_eq!(parse_gamesir_frame(&frame), Some(GameSirBattery::Charging));

        // Unsolicited F4 00 frame must be ignored
        let mut f4 = vec![0u8; 65];
        f4[1] = 0xF4;
        f4[2] = 0x00;
        f4[19] = 50;
        assert_eq!(parse_gamesir_frame(&f4), None);

        // Out-of-range battery value must be ignored
        frame[19] = 150;
        assert_eq!(parse_gamesir_frame(&frame), None);
    }

    #[test]
    fn maps_generic_zikway_string() {
        assert_eq!(
            friendly_gamesir_name(0x2106, "HID zkm"),
            "GameSir Nova 2 Lite"
        );
        assert_eq!(
            friendly_gamesir_name(0x2106, "GameSir Cyclone 2"),
            "GameSir Cyclone 2"
        );
    }
}
