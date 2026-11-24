use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::Local;

use crate::config::AppConfig;
use crate::model::{ConnectionType, DeviceKind};
use crate::plugins::runner::{LoadedPlugin, matches_bt_rule, matches_hid_rule};
use crate::plugins::schema::{
    BluetoothPluginOptions, ByteCondition, ChecksumKind, HidProtocolMode, HidProtocolSpec,
    LevelExtractSpec, ChargingExtractSpec, PluginMatchRule, PluginSpec, PluginTransport,
    format_hex_bytes, parse_hex_bytes,
};
use crate::providers::bluetooth::scan_bluetooth_devices;
use crate::transport::hid::{HidDeviceInfo, HidHandle, enumerate_hid_devices};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportStatus {
    BuiltIn(&'static str),
    Plugin(String),
    Unmatched,
}

impl SupportStatus {
    pub fn label(&self) -> String {
        match self {
            SupportStatus::BuiltIn(s) => format!("内置: {s}"),
            SupportStatus::Plugin(id) => format!("插件: {id}"),
            SupportStatus::Unmatched => "未适配".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScannedHidCollection {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub interface_number: Option<u8>,
    pub manufacturer: String,
    pub product: String,
    pub is_vendor_channel: bool,
    pub support: SupportStatus,
}

#[derive(Debug, Clone)]
pub struct ScannedHidGroup {
    pub vid: u16,
    pub pid: u16,
    pub display_name: String,
    pub manufacturer: String,
    pub has_vendor_channel: bool,
    pub support: SupportStatus,
    pub collections: Vec<ScannedHidCollection>,
}

#[derive(Debug, Clone)]
pub struct ScannedBtDevice {
    pub mac: String,
    pub name: String,
    pub is_le: bool,
    pub has_classic: bool,
    pub connected: bool,
    pub kind: DeviceKind,
    pub connection: ConnectionType,
    pub battery_percent: Option<u8>,
    pub support: SupportStatus,
}

#[derive(Debug, Clone, Default)]
pub struct ScanSnapshot {
    pub hid_groups: Vec<ScannedHidGroup>,
    pub bt_devices: Vec<ScannedBtDevice>,
    pub scanned_at: String,
}

#[derive(Debug, Clone)]
pub struct ProbeRequest {
    pub collection: ScannedHidCollection,
    pub mode: HidProtocolMode,
    pub request_hex: String,
    pub xor_ff: bool,
    pub timeout_ms: u32,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub ok: bool,
    pub raw_hex: String,
    pub decoded_hex: String,
    pub decoded_bytes: Vec<u8>,
    pub candidate_offsets: Vec<(usize, u8)>,
    pub message: String,
}

pub fn classify_builtin_hid(d: &HidDeviceInfo) -> Option<&'static str> {
    match d.vendor_id {
        0x5253 | 0x3837 | 0x291D | 0xA8A5 | 0x2023 => Some("MCHOSE"),
        0x3537 => Some("GameSir"),
        0x046D => Some("Logitech"),
        0x1532 => Some("Razer"),
        0x1038 => Some("SteelSeries"),
        0x03F0 | 0x0951 => Some("HyperX"),
        0x1B1C => Some("Corsair"),
        0x054C => Some("Sony"),
        0x057E => Some("Nintendo"),
        0x2DC8 => Some("8BitDo"),
        0x37D7 => Some("FlyDigi"),
        _ => {
            if d.usage_page == 0xFF02 && d.usage == 0x0002 {
                Some("CompX")
            } else if matches!(d.usage_page, 0x0084 | 0x0085) {
                Some("HID Battery")
            } else {
                None
            }
        }
    }
}

pub fn run_device_scan(plugins: &[LoadedPlugin]) -> ScanSnapshot {
    let raw_hid = enumerate_hid_devices();
    let mut by_vp: BTreeMap<(u16, u16), Vec<ScannedHidCollection>> = BTreeMap::new();

    for d in raw_hid {
        let plugin_hit = plugins
            .iter()
            .find(|p| p.spec.enabled && p.spec.transport == PluginTransport::Hid && matches_hid_rule(&d, &p.spec.matcher))
            .map(|p| SupportStatus::Plugin(p.spec.id.clone()));
        let support = plugin_hit
            .or_else(|| classify_builtin_hid(&d).map(SupportStatus::BuiltIn))
            .unwrap_or(SupportStatus::Unmatched);
        let is_vendor_channel =
            d.usage_page >= 0xFF00 || matches!(d.usage_page, 0x0084 | 0x0085);

        by_vp
            .entry((d.vendor_id, d.product_id))
            .or_default()
            .push(ScannedHidCollection {
                path: d.path,
                vid: d.vendor_id,
                pid: d.product_id,
                usage_page: d.usage_page,
                usage: d.usage,
                input_len: d.input_report_len,
                output_len: d.output_report_len,
                feature_len: d.feature_report_len,
                interface_number: d.interface_number,
                manufacturer: d.manufacturer,
                product: d.product,
                is_vendor_channel,
                support,
            });
    }

    let mut hid_groups = Vec::with_capacity(by_vp.len());
    for ((vid, pid), mut cols) in by_vp {
        cols.sort_by_key(|c| {
            (
                !c.is_vendor_channel,
                std::cmp::Reverse(c.usage_page),
                c.interface_number.unwrap_or(0xFF),
                c.usage,
            )
        });

        let has_vendor_channel = cols.iter().any(|c| c.is_vendor_channel);
        let display_name = cols
            .iter()
            .map(|c| c.product.trim())
            .find(|s| !s.is_empty() && !s.eq_ignore_ascii_case("USB Input Device"))
            .or_else(|| cols.iter().map(|c| c.product.trim()).find(|s| !s.is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| format!("HID 设备 {vid:04X}:{pid:04X}"));
        let manufacturer = cols
            .iter()
            .map(|c| c.manufacturer.trim())
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();

        let group_support = cols
            .iter()
            .find_map(|c| match &c.support {
                SupportStatus::Plugin(id) => Some(SupportStatus::Plugin(id.clone())),
                _ => None,
            })
            .or_else(|| {
                cols.iter().find_map(|c| match &c.support {
                    SupportStatus::BuiltIn(b) => Some(SupportStatus::BuiltIn(b)),
                    _ => None,
                })
            })
            .unwrap_or(SupportStatus::Unmatched);

        hid_groups.push(ScannedHidGroup {
            vid,
            pid,
            display_name,
            manufacturer,
            has_vendor_channel,
            support: group_support,
            collections: cols,
        });
    }

    hid_groups.sort_by(|a, b| {
        b.has_vendor_channel
            .cmp(&a.has_vendor_channel)
            .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()))
            .then_with(|| a.vid.cmp(&b.vid))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    let raw_bt = scan_bluetooth_devices();
    let mut bt_devices: Vec<ScannedBtDevice> = raw_bt
        .into_iter()
        .map(|b| {
            let plugin_hit = plugins
                .iter()
                .find(|p| {
                    p.spec.enabled
                        && p.spec.transport == PluginTransport::Bluetooth
                        && matches_bt_rule(&b.name, &b.mac, &p.spec.matcher)
                })
                .map(|p| SupportStatus::Plugin(p.spec.id.clone()));
            let support = plugin_hit.unwrap_or_else(|| {
                if b.battery_percent.is_some() {
                    SupportStatus::BuiltIn("Windows BT")
                } else {
                    SupportStatus::Unmatched
                }
            });
            ScannedBtDevice {
                mac: b.mac,
                name: b.name,
                is_le: b.is_le,
                has_classic: b.has_classic,
                connected: b.connected,
                kind: b.kind,
                connection: b.connection,
                battery_percent: b.battery_percent,
                support,
            }
        })
        .collect();
    bt_devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    ScanSnapshot {
        hid_groups,
        bt_devices,
        scanned_at: Local::now().format("%H:%M:%S").to_string(),
    }
}

pub fn extract_candidate_battery_offsets(bytes: &[u8]) -> Vec<(usize, u8)> {
    bytes
        .iter()
        .copied()
        .enumerate()
        .filter(|&(idx, b)| idx >= 1 && (1..=100).contains(&b))
        .take(18)
        .collect()
}

pub fn run_hid_probe(req: &ProbeRequest) -> ProbeResult {
    let fail = |msg: String| ProbeResult {
        ok: false,
        raw_hex: String::new(),
        decoded_hex: String::new(),
        decoded_bytes: Vec::new(),
        candidate_offsets: Vec::new(),
        message: msg,
    };

    let col = &req.collection;
    let raw_pkt = match req.mode {
        HidProtocolMode::PassiveInput => {
            let Ok(h) = HidHandle::open_read_only(&col.path, true) else {
                return fail(format!(
                    "无法以只读打开 {:04X}:{:04X} (UP={:04X})",
                    col.vid, col.pid, col.usage_page
                ));
            };
            let len = (col.input_len as usize).max(64);
            let mut got = None;
            for _ in 0..4 {
                if let Ok(Some(pkt)) = h.read_overlapped(len, 180) {
                    got = Some(pkt);
                    break;
                }
            }
            got
        }
        HidProtocolMode::FeatureReport => {
            let mut bytes = match parse_hex_bytes(&req.request_hex) {
                Ok(b) if !b.is_empty() => b,
                Ok(_) => return fail("Feature 模式请输入至少 1 个字节的 Report ID（如 11 06）".into()),
                Err(e) => return fail(format!("指令格式错误: {e}")),
            };
            let rid = bytes[0];
            let target_len = (col.feature_len as usize).max(bytes.len()).max(2);
            bytes.resize(target_len, 0);
            if req.xor_ff && bytes.len() > 1 {
                for b in &mut bytes[1..] {
                    *b ^= 0xFF;
                }
            }
            let Ok(h) = HidHandle::open(&col.path, false) else {
                return fail("打开 HID 设备失败（可能被驱动独占）".into());
            };
            if let Err(e) = h.set_feature(&bytes) {
                return fail(format!("HidD_SetFeature 失败: {e}"));
            }
            thread::sleep(Duration::from_millis(55));
            match h.get_feature(rid, target_len) {
                Ok(resp) => Some(resp),
                Err(e) => return fail(format!("HidD_GetFeature 失败: {e}")),
            }
        }
        HidProtocolMode::OutputInput | HidProtocolMode::OutputWithFeatureFallback => {
            let mut bytes = match parse_hex_bytes(&req.request_hex) {
                Ok(b) if !b.is_empty() => b,
                Ok(_) => return fail("请输入测试指令字节（如 55 65 01 或 00 01 01）".into()),
                Err(e) => return fail(format!("指令格式错误: {e}")),
            };
            let target_len = (col.output_len as usize).max(bytes.len()).max(2);
            bytes.resize(target_len, 0);
            if req.xor_ff && bytes.len() > 1 {
                for b in &mut bytes[1..] {
                    *b ^= 0xFF;
                }
            }
            let Ok(h) = HidHandle::open(&col.path, true) else {
                return fail("打开 HID 设备失败（系统独占接口或无权限）".into());
            };
            let in_len = (col.input_len as usize).max(bytes.len()).max(64);
            match h.write_and_read_queued(&bytes, in_len, req.timeout_ms.clamp(150, 2000), 6, |pkt| {
                pkt.iter().any(|&b| b != 0)
            }) {
                Ok(Some(pkt)) => Some(pkt),
                Ok(None) => None,
                Err(e) => {
                    if req.mode == HidProtocolMode::OutputWithFeatureFallback
                        && h.set_feature(&bytes).is_ok()
                    {
                        h.read_overlapped(in_len, 350).ok().flatten()
                    } else {
                        return fail(format!("HID 读写失败: {e}"));
                    }
                }
            }
        }
    };

    let Some(raw) = raw_pkt else {
        return fail("设备无应答（可能处于休眠、未开机，或需要更换通道/指令）".into());
    };

    let mut decoded = raw.clone();
    if req.xor_ff && decoded.len() > 1 {
        for b in &mut decoded[1..] {
            *b ^= 0xFF;
        }
    }
    let candidates = extract_candidate_battery_offsets(&decoded);
    ProbeResult {
        ok: true,
        raw_hex: format_hex_bytes(&raw),
        decoded_hex: format_hex_bytes(&decoded),
        candidate_offsets: candidates,
        decoded_bytes: decoded,
        message: format!("收到 {} 字节响应，点击下方高亮候选字节即可填入电量偏移", raw.len()),
    }
}

pub fn sanitize_plugin_id(raw: &str) -> String {
    let slug: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        format!("custom-{}", Local::now().format("%H%M%S"))
    } else {
        trimmed.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_hid_plugin_from_scan(
    col: &ScannedHidCollection,
    name: String,
    kind: DeviceKind,
    mode: HidProtocolMode,
    request_hex: String,
    xor_ff: bool,
    level_offset: usize,
    charging_offset: Option<usize>,
    sample_decoded: &[u8],
) -> PluginSpec {
    let base_id = format!("user-{:04x}-{:04x}", col.vid, col.pid);
    let clean_name = if name.trim().is_empty() {
        if col.product.trim().is_empty() {
            format!("自定义设备 {:04X}:{:04X}", col.vid, col.pid)
        } else {
            col.product.trim().to_string()
        }
    } else {
        name.trim().to_string()
    };

    let mut response_match = Vec::new();
    if let Some(&b0) = sample_decoded.first()
        && b0 != 0
    {
        response_match.push(ByteCondition {
            offset: 0,
            equals: b0,
            mask: None,
        });
    }
    if let Some(&b1) = sample_decoded.get(1)
        && b1 != 0
    {
        response_match.push(ByteCondition {
            offset: 1,
            equals: b1,
            mask: None,
        });
    }

    let report_len = match mode {
        HidProtocolMode::FeatureReport => (col.feature_len as usize).max(2),
        HidProtocolMode::PassiveInput => (col.input_len as usize).max(2),
        HidProtocolMode::OutputInput | HidProtocolMode::OutputWithFeatureFallback => {
            (col.output_len as usize).max(col.input_len as usize).max(2)
        }
    };
    let report_id = parse_hex_bytes(&request_hex)
        .ok()
        .and_then(|v| v.first().copied());

    PluginSpec {
        schema_version: 1,
        id: base_id,
        name: clean_name,
        brand: (!col.manufacturer.trim().is_empty()).then(|| col.manufacturer.trim().to_string()),
        kind,
        connection: ConnectionType::Dongle24G,
        enabled: true,
        priority: 150,
        transport: PluginTransport::Hid,
        matcher: PluginMatchRule {
            vid: Some(col.vid),
            pids: vec![col.pid],
            usage_page: Some(col.usage_page),
            usage: Some(col.usage),
            interface_number: col.interface_number,
            ..Default::default()
        },
        protocol: Some(HidProtocolSpec {
            mode,
            report_id,
            report_len,
            request_hex: request_hex.trim().to_string(),
            request_xor_mask: xor_ff.then_some(0xFF),
            request_xor_start: 1,
            checksum: ChecksumKind::None,
            delay_ms: 50,
            timeout_ms: 600,
            retries: 3,
            max_reads: 8,
            response_xor_mask: xor_ff.then_some(0xFF),
            response_xor_start: 1,
            response_min_len: (level_offset + 1).max(2),
            response_match,
            response_ignore: Vec::new(),
            offline_match: Vec::new(),
            level: LevelExtractSpec {
                offset: level_offset,
                ..Default::default()
            },
            charging: charging_offset.map(|offset| ChargingExtractSpec {
                offset,
                nonzero: true,
                ..Default::default()
            }),
        }),
        bt_options: None,
    }
}

pub fn build_bt_plugin_from_scan(bt: &ScannedBtDevice, kind: DeviceKind) -> PluginSpec {
    let short_mac: String = bt.mac.chars().take(6).collect();
    PluginSpec {
        schema_version: 1,
        id: sanitize_plugin_id(&format!("bt-{}-{}", bt.name, short_mac)),
        name: bt.name.clone(),
        brand: None,
        kind,
        connection: bt.connection,
        enabled: true,
        priority: 120,
        transport: PluginTransport::Bluetooth,
        matcher: PluginMatchRule {
            name_contains: Some(bt.name.clone()),
            mac_prefix: Some(bt.mac.clone()),
            ..Default::default()
        },
        protocol: None,
        bt_options: Some(BluetoothPluginOptions {
            gatt_service_uuid: bt.is_le.then(|| "180F".to_string()),
            gatt_char_uuid: bt.is_le.then(|| "2A19".to_string()),
            level_offset: 0,
            scale_max: 100,
            override_kind: true,
            ignore_device: false,
        }),
    }
}

pub fn save_plugin_to_user_dir(spec: &PluginSpec) -> Result<PathBuf> {
    if spec.id.trim().is_empty() {
        bail!("插件 ID 不能为空");
    }
    let dir = AppConfig::plugins_dir();
    fs::create_dir_all(&dir)?;
    let filename = format!("{}.json", sanitize_plugin_id(&spec.id));
    let path = dir.join(filename);
    let json = serde_json::to_string_pretty(spec)?;
    fs::write(&path, json)?;
    Ok(path)
}

pub fn delete_user_plugin(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_prioritizes_vendor_channels() {
        let col = ScannedHidCollection {
            path: "p".into(),
            vid: 0x1234,
            pid: 0x5678,
            usage_page: 0xFF01,
            usage: 0x0001,
            input_len: 64,
            output_len: 64,
            feature_len: 21,
            interface_number: Some(2),
            manufacturer: "Acme".into(),
            product: "Acme Mouse".into(),
            is_vendor_channel: true,
            support: SupportStatus::Unmatched,
        };
        let spec = build_hid_plugin_from_scan(
            &col,
            "我的鼠标".into(),
            DeviceKind::Mouse,
            HidProtocolMode::FeatureReport,
            "11 06".into(),
            true,
            11,
            Some(12),
            &[0x11, 0x06, 0x53, 0x52],
        );
        assert_eq!(spec.id, "user-1234-5678");
        assert_eq!(spec.matcher.vid, Some(0x1234));
        assert_eq!(spec.matcher.pids, vec![0x5678]);
        let proto = spec.protocol.unwrap();
        assert_eq!(proto.request_xor_mask, Some(0xFF));
        assert_eq!(proto.level.offset, 11);
        assert_eq!(proto.charging.unwrap().offset, 12);
        assert_eq!(proto.response_match.len(), 2);

        let candidates = extract_candidate_battery_offsets(&[0x11, 0x06, 0xFF, 0x4E, 0x00, 100]);
        assert!(candidates.contains(&(1, 0x06)));
        assert!(candidates.contains(&(3, 0x4E)));
        assert!(candidates.contains(&(5, 100)));
    }
}
