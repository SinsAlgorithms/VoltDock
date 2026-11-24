use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;

use crate::config::AppConfig;
use crate::model::{ConnectionType, DeviceReading, PowerState};
use crate::plugins::schema::{
    ByteCondition, ChecksumKind, HidProtocolMode, HidProtocolSpec, PluginMatchRule, PluginSpec,
    PluginTransport, parse_hex_bytes,
};
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::{HidDeviceInfo, HidHandle};

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub path: PathBuf,
    pub spec: PluginSpec,
    pub is_user_dir: bool,
}

#[derive(Debug, Clone)]
pub struct PluginLoadError {
    pub path: PathBuf,
    pub error: String,
}

pub fn plugin_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![AppConfig::plugins_dir()];
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let exe_plugins = parent.join("plugins");
        if !dirs.contains(&exe_plugins) {
            dirs.push(exe_plugins);
        }
    }
    dirs
}

pub fn example_plugin_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "mchose_a7_ultra.json.example",
            include_str_or_mchose_example(),
        ),
        (
            "gamesir_nova2_lite.json.example",
            include_str_or_gamesir_example(),
        ),
        (
            "compx_mouse.json.example",
            include_str_or_compx_example(),
        ),
        (
            "ble_custom_gatt.json.example",
            include_str_or_ble_example(),
        ),
    ]
}

fn include_str_or_mchose_example() -> &'static str {
    r#"{
  "schema_version": 1,
  "id": "custom-mchose-a7",
  "name": "MCHOSE A7 Ultra (示例插件)",
  "brand": "MCHOSE",
  "kind": "Mouse",
  "connection": "Dongle24G",
  "enabled": true,
  "priority": 120,
  "transport": "hid",
  "match": {
    "vid": "0x5253",
    "pids": ["0x1020"],
    "usage_page": "0xFF01",
    "usage": "0x0001"
  },
  "protocol": {
    "mode": "feature_report",
    "report_id": "0x11",
    "report_len": 21,
    "request_hex": "11 06",
    "request_xor_mask": "0xFF",
    "request_xor_start": 1,
    "checksum": "none",
    "delay_ms": 60,
    "timeout_ms": 400,
    "retries": 3,
    "response_xor_mask": "0xFF",
    "response_xor_start": 1,
    "response_min_len": 13,
    "response_match": [
      { "offset": 0, "equals": "0x11" },
      { "offset": 1, "equals": "0x06" },
      { "offset": 2, "equals": "0x53" },
      { "offset": 3, "equals": "0x52" }
    ],
    "level": {
      "offset": 11,
      "mask": "0xFF",
      "scale_max": 100,
      "min_valid": 0,
      "max_valid": 100
    },
    "charging": {
      "offset": 12,
      "mask": "0xFF",
      "nonzero": true
    }
  }
}
"#
}

fn include_str_or_gamesir_example() -> &'static str {
    r#"{
  "schema_version": 1,
  "id": "custom-gamesir-nova2",
  "name": "GameSir Nova 2 Lite (示例插件)",
  "brand": "GameSir",
  "kind": "Gamepad",
  "connection": "Dongle24G",
  "enabled": true,
  "priority": 120,
  "transport": "hid",
  "match": {
    "vid": "0x3537",
    "pids": ["0x2106"],
    "usage_page": "0xFF7A",
    "min_input_len": 65,
    "min_output_len": 65
  },
  "protocol": {
    "mode": "output_input",
    "report_len": 65,
    "request_hex": "00 01 01",
    "timeout_ms": 1000,
    "max_reads": 10,
    "response_min_len": 20,
    "response_match": [
      { "offset": 1, "equals": "0x01" }
    ],
    "response_ignore": [
      [
        { "offset": 1, "equals": "0xF4" },
        { "offset": 2, "equals": "0x00" }
      ]
    ],
    "level": {
      "offset": 19,
      "mask": "0xFF",
      "scale_max": 100,
      "min_valid": 0,
      "max_valid": 100,
      "charging_special_value": "0xFF"
    }
  }
}
"#
}

fn include_str_or_compx_example() -> &'static str {
    r#"{
  "schema_version": 1,
  "id": "custom-compx-mouse",
  "name": "CompX 方案无线鼠标 (VGN/ATK/蜻蜓等示例)",
  "brand": "CompX",
  "kind": "Mouse",
  "connection": "Dongle24G",
  "enabled": true,
  "priority": 100,
  "transport": "hid",
  "match": {
    "usage_page": "0xFF02",
    "usage": "0x0002",
    "min_output_len": 17
  },
  "protocol": {
    "mode": "output_input",
    "report_len": 17,
    "request_hex": "08 04",
    "checksum": "comp_x55",
    "timeout_ms": 350,
    "max_reads": 4,
    "response_min_len": 10,
    "response_match": [
      { "offset": 0, "equals": "0x08" },
      { "offset": 1, "equals": "0x04" }
    ],
    "level": {
      "offset": 6,
      "mask": "0xFF",
      "scale_max": 100,
      "min_valid": 0,
      "max_valid": 100
    },
    "charging": {
      "offset": 7,
      "mask": "0xFF",
      "nonzero": true
    }
  }
}
"#
}

fn include_str_or_ble_example() -> &'static str {
    r#"{
  "schema_version": 1,
  "id": "custom-ble-device",
  "name": "自定义蓝牙设备 (示例插件)",
  "brand": "Custom",
  "kind": "Keyboard",
  "connection": "BluetoothLE",
  "enabled": true,
  "priority": 100,
  "transport": "bluetooth",
  "match": {
    "name_contains": "MyBLEDevice",
    "mac_prefix": ""
  },
  "bt_options": {
    "gatt_service_uuid": "180F",
    "gatt_char_uuid": "2A19",
    "level_offset": 0,
    "scale_max": 100,
    "override_kind": true,
    "ignore_device": false
  }
}
"#
}

pub fn ensure_plugins_dir_and_examples() {
    let dir = AppConfig::plugins_dir();
    let examples_dir = dir.join("examples");
    if fs::create_dir_all(&examples_dir).is_err() {
        return;
    }
    for (fname, content) in example_plugin_files() {
        let p = examples_dir.join(fname);
        if !p.exists() {
            let _ = fs::write(p, content);
        }
    }
}

pub fn load_plugin_file(path: &Path, is_user_dir: bool) -> Result<LoadedPlugin> {
    let raw = fs::read_to_string(path)?;
    let mut spec: PluginSpec = serde_json::from_str(raw.trim_start_matches('\u{feff}'))?;
    if spec.id.trim().is_empty() {
        spec.id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom-plugin")
            .to_string();
    }
    if spec.name.trim().is_empty() {
        spec.name = spec.id.clone();
    }
    Ok(LoadedPlugin {
        path: path.to_path_buf(),
        spec,
        is_user_dir,
    })
}

pub fn load_all_plugins() -> (Vec<LoadedPlugin>, Vec<PluginLoadError>) {
    let user_dir = AppConfig::plugins_dir();
    let mut plugins = Vec::new();
    let mut errors = Vec::new();

    for dir in plugin_search_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let is_user_dir = dir == user_dir;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_json = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
            if !is_json {
                continue;
            }
            match load_plugin_file(&path, is_user_dir) {
                Ok(p) => plugins.push(p),
                Err(e) => errors.push(PluginLoadError {
                    path,
                    error: e.to_string(),
                }),
            }
        }
    }

    plugins.sort_by(|a, b| {
        b.spec
            .priority
            .cmp(&a.spec.priority)
            .then_with(|| a.spec.id.cmp(&b.spec.id))
    });
    (plugins, errors)
}

pub fn matches_hid_rule(info: &HidDeviceInfo, rule: &PluginMatchRule) -> bool {
    let has_selector = rule.vid.is_some()
        || rule.usage_page.is_some()
        || rule
            .product_contains
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
        || rule
            .manufacturer_contains
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty());
    if !has_selector {
        return false;
    }

    if let Some(vid) = rule.vid
        && info.vendor_id != vid
    {
        return false;
    }
    if !rule.pids.is_empty() && !rule.pids.contains(&info.product_id) {
        return false;
    }
    if let Some(up) = rule.usage_page
        && info.usage_page != up
    {
        return false;
    }
    if let Some(u) = rule.usage
        && info.usage != u
    {
        return false;
    }
    if let Some(mi) = rule.interface_number
        && info.interface_number != Some(mi)
    {
        return false;
    }
    if info.input_report_len < rule.min_input_len
        || info.output_report_len < rule.min_output_len
        || info.feature_report_len < rule.min_feature_len
    {
        return false;
    }
    if let Some(sub) = rule.product_contains.as_deref()
        && !sub.trim().is_empty()
        && !info
            .product
            .to_ascii_lowercase()
            .contains(&sub.trim().to_ascii_lowercase())
    {
        return false;
    }
    if let Some(sub) = rule.manufacturer_contains.as_deref()
        && !sub.trim().is_empty()
        && !info
            .manufacturer
            .to_ascii_lowercase()
            .contains(&sub.trim().to_ascii_lowercase())
    {
        return false;
    }
    true
}

pub fn matches_bt_rule(name: &str, mac: &str, rule: &PluginMatchRule) -> bool {
    let has_name = rule
        .name_contains
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let has_mac = rule
        .mac_prefix
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    if !has_name && !has_mac {
        return false;
    }
    if let Some(sub) = rule.name_contains.as_deref()
        && !sub.trim().is_empty()
        && !name
            .to_ascii_lowercase()
            .contains(&sub.trim().to_ascii_lowercase())
    {
        return false;
    }
    if let Some(pref) = rule.mac_prefix.as_deref()
        && !pref.trim().is_empty()
    {
        let clean_pref = pref.replace([':', '-', ' '], "").to_ascii_uppercase();
        let clean_mac = mac.replace([':', '-', ' '], "").to_ascii_uppercase();
        if !clean_mac.starts_with(&clean_pref) {
            return false;
        }
    }
    true
}

pub fn build_plugin_request(proto: &HidProtocolSpec, cap_len: usize) -> Result<Vec<u8>> {
    let mut buf = parse_hex_bytes(&proto.request_hex)?;
    let target_len = proto.report_len.max(cap_len).max(buf.len()).max(1);
    buf.resize(target_len, 0);

    match proto.checksum {
        ChecksumKind::None => {}
        ChecksumKind::XorLast => {
            if target_len >= 2 {
                let xor = buf[..target_len - 1].iter().fold(0u8, |a, &b| a ^ b);
                buf[target_len - 1] = xor;
            }
        }
        ChecksumKind::SumLast => {
            if target_len >= 2 {
                let sum = buf[..target_len - 1]
                    .iter()
                    .fold(0u8, |a, &b| a.wrapping_add(b));
                buf[target_len - 1] = sum;
            }
        }
        ChecksumKind::CompX55 => {
            if target_len >= 2 {
                let sum = buf[..target_len - 1]
                    .iter()
                    .fold(0u8, |a, &b| a.wrapping_add(b));
                buf[target_len - 1] = 0x55u8.wrapping_sub(sum);
            }
        }
    }

    if let Some(mask) = proto.request_xor_mask {
        let start = proto.request_xor_start.min(buf.len());
        for b in &mut buf[start..] {
            *b ^= mask;
        }
    }
    Ok(buf)
}

pub fn decode_plugin_response(proto: &HidProtocolSpec, raw: &[u8]) -> Vec<u8> {
    let mut buf = raw.to_vec();
    if let Some(mask) = proto.response_xor_mask {
        let start = proto.response_xor_start.min(buf.len());
        for b in &mut buf[start..] {
            *b ^= mask;
        }
    }
    buf
}

fn conditions_all_match(conds: &[ByteCondition], buf: &[u8]) -> bool {
    conds.iter().all(|c| c.matches(buf))
}

pub fn parse_plugin_response(
    proto: &HidProtocolSpec,
    raw: &[u8],
) -> Option<(Option<u8>, PowerState)> {
    if raw.len() < proto.response_min_len {
        return None;
    }
    let buf = decode_plugin_response(proto, raw);

    for ignore_rule in &proto.response_ignore {
        if !ignore_rule.is_empty() && conditions_all_match(ignore_rule, &buf) {
            return None;
        }
    }

    if !conditions_all_match(&proto.response_match, &buf) {
        return None;
    }

    if !proto.offline_match.is_empty() && conditions_all_match(&proto.offline_match, &buf) {
        return Some((None, PowerState::Offline));
    }

    let &raw_lvl_byte = buf.get(proto.level.offset)?;
    let shift = proto.level.shift_right.min(7);
    let val = (raw_lvl_byte & proto.level.mask) >> shift;

    if let Some(special) = proto.level.charging_special_value
        && (val == special || raw_lvl_byte == special)
    {
        return Some((None, PowerState::Charging));
    }

    if val < proto.level.min_valid || val > proto.level.max_valid {
        return None;
    }

    let pct = if proto.level.scale_max > 0 && proto.level.scale_max != 100 {
        ((val as f64 / proto.level.scale_max as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    } else {
        val.min(100)
    };

    let state = if let Some(cs) = &proto.charging {
        if let Some(&cb) = buf.get(cs.offset) {
            let cshift = cs.shift_right.min(7);
            let cval = (cb & cs.mask) >> cshift;
            if cs.full_equals == Some(cval) {
                PowerState::Full
            } else if let Some(eq) = cs.equals {
                if cval == eq {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                }
            } else if cs.nonzero {
                if cval != 0 {
                    PowerState::Charging
                } else {
                    PowerState::Discharging
                }
            } else {
                PowerState::Discharging
            }
        } else {
            PowerState::Discharging
        }
    } else if pct == 100 {
        PowerState::Full
    } else {
        PowerState::Discharging
    };

    Some((Some(pct), state))
}

fn query_hid_plugin_collection(
    info: &HidDeviceInfo,
    proto: &HidProtocolSpec,
    diag: &mut Vec<String>,
    plugin_id: &str,
) -> Option<(Option<u8>, PowerState)> {
    let retries = proto.retries.clamp(1, 8);
    let timeout_ms = proto.timeout_ms.clamp(60, 4000);
    let max_reads = proto.max_reads.clamp(1, 24);

    match proto.mode {
        HidProtocolMode::PassiveInput => {
            let handle = HidHandle::open_read_only(&info.path, true).ok()?;
            let in_len = (info.input_report_len as usize)
                .max(proto.report_len)
                .max(64);
            for _ in 0..max_reads {
                if let Ok(Some(pkt)) = handle.read_overlapped(in_len, timeout_ms.min(400))
                    && let Some(res) = parse_plugin_response(proto, &pkt)
                {
                    return Some(res);
                }
            }
            None
        }
        HidProtocolMode::FeatureReport => {
            let cap_len = (info.feature_report_len as usize).max(1);
            let req = match build_plugin_request(proto, cap_len) {
                Ok(r) => r,
                Err(e) => {
                    diag.push(format!("[plugin:{plugin_id}] bad request_hex: {e}"));
                    return None;
                }
            };
            let rid = proto
                .report_id
                .unwrap_or_else(|| req.first().copied().unwrap_or(0));
            let read_len = req.len().max(cap_len);
            let handle = match HidHandle::open(&info.path, false) {
                Ok(h) => h,
                Err(e) => {
                    diag.push(format!(
                        "[plugin:{plugin_id}] open {:04X}:{:04X} failed: {e}",
                        info.vendor_id, info.product_id
                    ));
                    return None;
                }
            };
            for _ in 0..retries {
                if let Err(e) = handle.set_feature(&req) {
                    diag.push(format!("[plugin:{plugin_id}] set_feature err: {e}"));
                    break;
                }
                if proto.delay_ms > 0 {
                    thread::sleep(Duration::from_millis(proto.delay_ms.min(500)));
                }
                match handle.get_feature(rid, read_len) {
                    Ok(resp) => {
                        if let Some(parsed) = parse_plugin_response(proto, &resp) {
                            return Some(parsed);
                        }
                    }
                    Err(e) => {
                        diag.push(format!("[plugin:{plugin_id}] get_feature err: {e}"));
                        break;
                    }
                }
            }
            None
        }
        HidProtocolMode::OutputInput | HidProtocolMode::OutputWithFeatureFallback => {
            let cap_out = (info.output_report_len as usize).max(1);
            let req = match build_plugin_request(proto, cap_out) {
                Ok(r) => r,
                Err(e) => {
                    diag.push(format!("[plugin:{plugin_id}] bad request_hex: {e}"));
                    return None;
                }
            };
            let in_len = (info.input_report_len as usize)
                .max(proto.report_len)
                .max(req.len())
                .max(64);
            let handle = match HidHandle::open(&info.path, true) {
                Ok(h) => h,
                Err(e) => {
                    diag.push(format!(
                        "[plugin:{plugin_id}] open {:04X}:{:04X} failed: {e}",
                        info.vendor_id, info.product_id
                    ));
                    return None;
                }
            };

            let mut parsed = None;
            let res = handle.write_and_read_queued(&req, in_len, timeout_ms, max_reads, |pkt| {
                if let Some(r) = parse_plugin_response(proto, pkt) {
                    parsed = Some(r);
                    true
                } else {
                    false
                }
            });
            match res {
                Ok(_) if parsed.is_some() => parsed,
                Ok(_) => None,
                Err(err)
                    if proto.mode == HidProtocolMode::OutputWithFeatureFallback
                        && (err.to_string().contains("err=1")
                            || err.to_string().contains("incorrect function")) =>
                {
                    if handle.set_feature(&req).is_ok() {
                        for _ in 0..max_reads.min(4) {
                            if let Ok(Some(pkt)) = handle.read_overlapped(in_len, timeout_ms / 2)
                                && let Some(r) = parse_plugin_response(proto, &pkt)
                            {
                                return Some(r);
                            }
                        }
                    }
                    None
                }
                Err(e) => {
                    diag.push(format!("[plugin:{plugin_id}] I/O err: {e}"));
                    None
                }
            }
        }
    }
}

pub struct PluginHidProvider {
    pub plugins: Arc<Vec<LoadedPlugin>>,
}

impl BatteryProvider for PluginHidProvider {
    fn name(&self) -> &'static str {
        "plugin_hid"
    }

    fn poll(&self, ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        let mut out = Vec::new();
        for loaded in self.plugins.iter() {
            let spec = &loaded.spec;
            if !spec.enabled || spec.transport != PluginTransport::Hid {
                continue;
            }
            let Some(proto) = &spec.protocol else {
                continue;
            };

            let mut grouped: BTreeMap<(u16, u16), Vec<&HidDeviceInfo>> = BTreeMap::new();
            for d in ctx.hid_devices {
                if matches_hid_rule(d, &spec.matcher) {
                    grouped
                        .entry((d.vendor_id, d.product_id))
                        .or_default()
                        .push(d);
                }
            }

            for ((vid, pid), cols) in grouped {
                let mut parsed: Option<(Option<u8>, PowerState)> = None;
                for col in &cols {
                    if let Some(res) = query_hid_plugin_collection(col, proto, diag, &spec.id) {
                        parsed = Some(res);
                        break;
                    }
                }

                let prod_name = cols
                    .iter()
                    .map(|c| c.product.trim())
                    .find(|s| !s.is_empty() && !s.eq_ignore_ascii_case("USB Input Device"));
                let name = if spec.name.trim().is_empty() {
                    prod_name
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("自定义设备 {vid:04X}:{pid:04X}"))
                } else {
                    spec.name.clone()
                };

                let dev_id = format!("plugin:{}:{vid:04x}:{pid:04x}", spec.id);
                if let Some((pct, st)) = parsed {
                    diag.push(format!(
                        "[plugin:{}] {vid:04X}:{pid:04X} '{name}' -> {:?}% st={st:?}",
                        spec.id, pct
                    ));
                    let conn = if st == PowerState::Charging
                        && spec.connection == ConnectionType::Dongle24G
                    {
                        ConnectionType::Wired
                    } else {
                        spec.connection
                    };
                    out.push(DeviceReading {
                        id: dev_id,
                        name,
                        brand: spec.brand.clone(),
                        kind: spec.kind,
                        connection: conn,
                        battery_percent: pct,
                        power_state: st,
                        provider: format!("plugin:{}", spec.id),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: Some(format!("插件: {}", spec.id)),
                    });
                } else {
                    diag.push(format!(
                        "[plugin:{}] {vid:04X}:{pid:04X} '{name}' offline/no reply",
                        spec.id
                    ));
                    out.push(DeviceReading {
                        id: dev_id,
                        name,
                        brand: spec.brand.clone(),
                        kind: spec.kind,
                        connection: spec.connection,
                        battery_percent: None,
                        power_state: PowerState::Offline,
                        provider: format!("plugin:{}", spec.id),
                        vid_pid: Some((vid, pid)),
                        mac_address: None,
                        last_seen: Local::now(),
                        approximate: false,
                        detail: Some("接收器在线，设备未开机/未连接".to_string()),
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;

    #[test]
    fn builds_and_parses_mchose_xor_and_compx_checksum() {
        let mchose: PluginSpec = serde_json::from_str(include_str_or_mchose_example()).unwrap();
        let m_proto = mchose.protocol.as_ref().unwrap();
        let req = build_plugin_request(m_proto, 21).unwrap();
        assert_eq!(req.len(), 21);
        assert_eq!(req[0], 0x11);
        assert_eq!(req[1], 0x06 ^ 0xFF);
        assert_eq!(req[2], 0xFF);

        // Captured live A7 Ultra frame (79% discharging)
        let decoded_payload = [
            0x53u8, 0x52, 0x11, 0x00, 0x05, 0x00, 0x07, 0x04, 0x09, 0x4f, 0x00, 0x51,
        ];
        let mut raw = vec![0x11, 0x06 ^ 0xFF];
        raw.extend(decoded_payload.iter().map(|b| b ^ 0xFF));
        assert_eq!(
            parse_plugin_response(m_proto, &raw),
            Some((Some(79), PowerState::Discharging))
        );

        let compx: PluginSpec = serde_json::from_str(include_str_or_compx_example()).unwrap();
        let c_proto = compx.protocol.as_ref().unwrap();
        let c_req = build_plugin_request(c_proto, 17).unwrap();
        assert_eq!(c_req.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0x55);
        assert_eq!(
            parse_plugin_response(c_proto, &[0x08, 0x04, 0, 0, 0, 0, 64, 1, 0, 0]),
            Some((Some(64), PowerState::Charging))
        );
    }

    #[test]
    fn parses_scaled_and_charging_special_value() {
        let gamesir: PluginSpec = serde_json::from_str(include_str_or_gamesir_example()).unwrap();
        let g_proto = gamesir.protocol.as_ref().unwrap();
        let mut frame = vec![0u8; 65];
        frame[1] = 0x01;
        frame[19] = 0xFF;
        assert_eq!(
            parse_plugin_response(g_proto, &frame),
            Some((None, PowerState::Charging))
        );

        // Ignored unsolicited F4 00 frame
        let mut ignored = vec![0u8; 65];
        ignored[1] = 0xF4;
        ignored[2] = 0x00;
        ignored[19] = 55;
        assert_eq!(parse_plugin_response(g_proto, &ignored), None);

        // Nibble + scale_max=5 (0..5 -> 0..100%)
        let json = r#"{
            "id": "scaled",
            "name": "Scaled",
            "kind": "Gamepad",
            "match": { "vid": "0x1234" },
            "protocol": {
                "mode": "passive_input",
                "response_min_len": 4,
                "level": { "offset": 2, "mask": "0x0F", "scale_max": 5, "max_valid": 5 },
                "charging": { "offset": 2, "mask": "0xF0", "shift_right": 4, "equals": "0x01" }
            }
        }"#;
        let spec: PluginSpec = serde_json::from_str(json).unwrap();
        let proto = spec.protocol.unwrap();
        assert_eq!(
            parse_plugin_response(&proto, &[0x00, 0x00, 0x14, 0x00]),
            Some((Some(80), PowerState::Charging))
        );
    }

    #[test]
    fn hid_matcher_checks_vid_pid_up_mi() {
        let dev = HidDeviceInfo {
            path: r"\\?\hid#vid_5253&pid_1020&mi_02#".to_string(),
            vendor_id: 0x5253,
            product_id: 0x1020,
            version_number: 1,
            usage_page: 0xFF01,
            usage: 0x0001,
            input_report_len: 65,
            output_report_len: 65,
            feature_report_len: 65,
            manufacturer: "RealTek".to_string(),
            product: "MCHOSE A7 Ultra".to_string(),
            interface_number: Some(2),
        };
        let rule = PluginMatchRule {
            vid: Some(0x5253),
            pids: vec![0x1020],
            usage_page: Some(0xFF01),
            interface_number: Some(2),
            ..Default::default()
        };
        assert!(matches_hid_rule(&dev, &rule));
        assert!(!matches_hid_rule(&dev, &PluginMatchRule::default()));
        let mut wrong_mi = rule.clone();
        wrong_mi.interface_number = Some(1);
        assert!(!matches_hid_rule(&dev, &wrong_mi));
        assert_eq!(DeviceKind::Mouse, DeviceKind::Mouse);
    }
}
