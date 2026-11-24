use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::model::{ConnectionType, DeviceKind};

// ============================================================================
// Flexible Hex / Integer Serde Helpers
// ============================================================================

fn parse_u16_str(s: &str) -> std::result::Result<u16, String> {
    let trimmed = s.trim();
    let hex_part = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    u16::from_str_radix(hex_part, 16).map_err(|e| format!("invalid hex u16 '{s}': {e}"))
}

fn parse_u8_str(s: &str) -> std::result::Result<u8, String> {
    let trimmed = s.trim();
    let hex_part = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    u8::from_str_radix(hex_part, 16).map_err(|e| format!("invalid hex u8 '{s}': {e}"))
}

pub fn de_u16_hex<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<u16, D::Error> {
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = u16;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a u16 integer or hex string like \"0x5253\"")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<u16, E> {
            u16::try_from(v).map_err(|_| E::custom(format!("u16 out of range: {v}")))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<u16, E> {
            u16::try_from(v).map_err(|_| E::custom(format!("u16 out of range: {v}")))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<u16, E> {
            parse_u16_str(v).map_err(E::custom)
        }
    }
    deserializer.deserialize_any(V)
}

pub fn de_opt_u16_hex<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u16>, D::Error> {
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Option<u16>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null, a u16 integer, or a hex string")
        }
        fn visit_none<E: de::Error>(self) -> std::result::Result<Option<u16>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> std::result::Result<Option<u16>, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> std::result::Result<Option<u16>, D2::Error> {
            de_u16_hex(d).map(Some)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Option<u16>, E> {
            u16::try_from(v).map(Some).map_err(|_| E::custom(format!("u16 out of range: {v}")))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Option<u16>, E> {
            u16::try_from(v).map(Some).map_err(|_| E::custom(format!("u16 out of range: {v}")))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Option<u16>, E> {
            if v.trim().is_empty() {
                Ok(None)
            } else {
                parse_u16_str(v).map(Some).map_err(E::custom)
            }
        }
    }
    deserializer.deserialize_any(V)
}

pub fn ser_opt_u16_hex<S: Serializer>(
    val: &Option<u16>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match val {
        Some(v) => serializer.serialize_str(&format!("0x{v:04X}")),
        None => serializer.serialize_none(),
    }
}

pub fn de_u8_hex<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<u8, D::Error> {
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = u8;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a u8 integer or hex string like \"0xFF\"")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<u8, E> {
            u8::try_from(v).map_err(|_| E::custom(format!("u8 out of range: {v}")))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<u8, E> {
            u8::try_from(v).map_err(|_| E::custom(format!("u8 out of range: {v}")))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<u8, E> {
            parse_u8_str(v).map_err(E::custom)
        }
    }
    deserializer.deserialize_any(V)
}

pub fn ser_u8_hex<S: Serializer>(val: &u8, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("0x{val:02X}"))
}

pub fn de_opt_u8_hex<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u8>, D::Error> {
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Option<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null, a u8 integer, or a hex string")
        }
        fn visit_none<E: de::Error>(self) -> std::result::Result<Option<u8>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> std::result::Result<Option<u8>, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> std::result::Result<Option<u8>, D2::Error> {
            de_u8_hex(d).map(Some)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Option<u8>, E> {
            u8::try_from(v).map(Some).map_err(|_| E::custom(format!("u8 out of range: {v}")))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Option<u8>, E> {
            u8::try_from(v).map(Some).map_err(|_| E::custom(format!("u8 out of range: {v}")))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Option<u8>, E> {
            if v.trim().is_empty() {
                Ok(None)
            } else {
                parse_u8_str(v).map(Some).map_err(E::custom)
            }
        }
    }
    deserializer.deserialize_any(V)
}

pub fn ser_opt_u8_hex<S: Serializer>(
    val: &Option<u8>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match val {
        Some(v) => serializer.serialize_str(&format!("0x{v:02X}")),
        None => serializer.serialize_none(),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum U16OrHex {
    Num(u64),
    Str(String),
}

pub fn de_vec_u16_hex<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<u16>, D::Error> {
    let items: Vec<U16OrHex> = Vec::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            U16OrHex::Num(n) => {
                out.push(u16::try_from(n).map_err(|_| de::Error::custom(format!("u16 out of range: {n}")))?);
            }
            U16OrHex::Str(s) => {
                out.push(parse_u16_str(&s).map_err(de::Error::custom)?);
            }
        }
    }
    Ok(out)
}

pub fn ser_vec_u16_hex<S: Serializer>(
    vals: &[u16],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq as _;
    let mut seq = serializer.serialize_seq(Some(vals.len()))?;
    for v in vals {
        seq.serialize_element(&format!("0x{v:04X}"))?;
    }
    seq.end()
}

// ============================================================================
// Hex String & UUID Utilities
// ============================================================================

/// Parses space/comma/colon-separated hex byte tokens (e.g. `"11 06"`, `"0x55, 0x65, 01"`).
pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let has_delimiters = trimmed.contains(|c: char| c.is_whitespace() || c == ',' || c == ':');
    if !has_delimiters && trimmed.len() > 2 && !trimmed.starts_with("0x") && !trimmed.starts_with("0X") {
        // Compact continuous hex string like "556501"
        if !trimmed.len().is_multiple_of(2) {
            bail!("连续十六进制字符串长度必须为偶数: '{trimmed}'");
        }
        let mut out = Vec::with_capacity(trimmed.len() / 2);
        for chunk in trimmed.as_bytes().chunks(2) {
            let pair = std::str::from_utf8(chunk)?;
            let b = u8::from_str_radix(pair, 16)
                .map_err(|e| anyhow::anyhow!("无效字节 '{pair}': {e}"))?;
            out.push(b);
        }
        return Ok(out);
    }

    let mut out = Vec::new();
    for raw_tok in trimmed.split(|c: char| c.is_whitespace() || c == ',' || c == ':') {
        let tok = raw_tok.trim();
        if tok.is_empty() {
            continue;
        }
        let clean = tok
            .strip_prefix("0x")
            .or_else(|| tok.strip_prefix("0X"))
            .unwrap_or(tok);
        let b = u8::from_str_radix(clean, 16)
            .map_err(|e| anyhow::anyhow!("无效十六进制字节 '{tok}': {e}"))?;
        out.push(b);
    }
    Ok(out)
}

pub fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses either a 16-bit BLE UUID (`"180F"` / `"0x180F"`) or a 128-bit UUID string into `u128`.
pub fn parse_bluetooth_uuid(s: &str) -> Option<u128> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let short_str = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    if (short_str.len() == 4 || short_str.len() == 8)
        && short_str.chars().all(|c| c.is_ascii_hexdigit())
        && let Ok(short) = u32::from_str_radix(short_str, 16)
    {
        return Some(0x0000_0000_0000_1000_8000_0080_5F9B_34FB_u128 | ((short as u128) << 96));
    }

    let clean: String = trimmed.chars().filter(|c| *c != '-').collect();
    if clean.len() == 32 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return u128::from_str_radix(&clean, 16).ok();
    }
    None
}

// ============================================================================
// Plugin Schema Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginTransport {
    #[default]
    Hid,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HidProtocolMode {
    FeatureReport,
    #[default]
    OutputInput,
    OutputWithFeatureFallback,
    PassiveInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumKind {
    #[default]
    None,
    XorLast,
    SumLast,
    CompX55,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginMatchRule {
    #[serde(
        deserialize_with = "de_opt_u16_hex",
        serialize_with = "ser_opt_u16_hex"
    )]
    pub vid: Option<u16>,
    #[serde(
        deserialize_with = "de_vec_u16_hex",
        serialize_with = "ser_vec_u16_hex"
    )]
    pub pids: Vec<u16>,
    #[serde(
        deserialize_with = "de_opt_u16_hex",
        serialize_with = "ser_opt_u16_hex"
    )]
    pub usage_page: Option<u16>,
    #[serde(
        deserialize_with = "de_opt_u16_hex",
        serialize_with = "ser_opt_u16_hex"
    )]
    pub usage: Option<u16>,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub interface_number: Option<u8>,
    pub min_input_len: u16,
    pub min_output_len: u16,
    pub min_feature_len: u16,
    pub product_contains: Option<String>,
    pub manufacturer_contains: Option<String>,
    pub name_contains: Option<String>,
    pub mac_prefix: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ByteCondition {
    pub offset: usize,
    #[serde(deserialize_with = "de_u8_hex", serialize_with = "ser_u8_hex")]
    pub equals: u8,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub mask: Option<u8>,
}

impl ByteCondition {
    pub fn matches(&self, buf: &[u8]) -> bool {
        let Some(&b) = buf.get(self.offset) else {
            return false;
        };
        let m = self.mask.unwrap_or(0xFF);
        (b & m) == (self.equals & m)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelExtractSpec {
    pub offset: usize,
    #[serde(deserialize_with = "de_u8_hex", serialize_with = "ser_u8_hex")]
    pub mask: u8,
    pub shift_right: u8,
    pub scale_max: u16,
    pub min_valid: u8,
    pub max_valid: u8,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub charging_special_value: Option<u8>,
}

impl Default for LevelExtractSpec {
    fn default() -> Self {
        Self {
            offset: 0,
            mask: 0xFF,
            shift_right: 0,
            scale_max: 100,
            min_valid: 0,
            max_valid: 100,
            charging_special_value: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChargingExtractSpec {
    pub offset: usize,
    #[serde(deserialize_with = "de_u8_hex", serialize_with = "ser_u8_hex")]
    pub mask: u8,
    pub shift_right: u8,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub equals: Option<u8>,
    pub nonzero: bool,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub full_equals: Option<u8>,
}

impl Default for ChargingExtractSpec {
    fn default() -> Self {
        Self {
            offset: 0,
            mask: 0xFF,
            shift_right: 0,
            equals: None,
            nonzero: true,
            full_equals: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HidProtocolSpec {
    pub mode: HidProtocolMode,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub report_id: Option<u8>,
    pub report_len: usize,
    pub request_hex: String,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub request_xor_mask: Option<u8>,
    pub request_xor_start: usize,
    pub checksum: ChecksumKind,
    pub delay_ms: u64,
    pub timeout_ms: u32,
    pub retries: usize,
    pub max_reads: usize,
    #[serde(
        deserialize_with = "de_opt_u8_hex",
        serialize_with = "ser_opt_u8_hex"
    )]
    pub response_xor_mask: Option<u8>,
    pub response_xor_start: usize,
    pub response_min_len: usize,
    pub response_match: Vec<ByteCondition>,
    pub response_ignore: Vec<Vec<ByteCondition>>,
    pub offline_match: Vec<ByteCondition>,
    pub level: LevelExtractSpec,
    pub charging: Option<ChargingExtractSpec>,
}

impl Default for HidProtocolSpec {
    fn default() -> Self {
        Self {
            mode: HidProtocolMode::OutputInput,
            report_id: None,
            report_len: 0,
            request_hex: String::new(),
            request_xor_mask: None,
            request_xor_start: 1,
            checksum: ChecksumKind::None,
            delay_ms: 40,
            timeout_ms: 500,
            retries: 3,
            max_reads: 6,
            response_xor_mask: None,
            response_xor_start: 1,
            response_min_len: 4,
            response_match: Vec::new(),
            response_ignore: Vec::new(),
            offline_match: Vec::new(),
            level: LevelExtractSpec::default(),
            charging: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BluetoothPluginOptions {
    pub gatt_service_uuid: Option<String>,
    pub gatt_char_uuid: Option<String>,
    pub level_offset: usize,
    pub scale_max: u16,
    pub override_kind: bool,
    pub ignore_device: bool,
}

impl Default for BluetoothPluginOptions {
    fn default() -> Self {
        Self {
            gatt_service_uuid: None,
            gatt_char_uuid: None,
            level_offset: 0,
            scale_max: 100,
            override_kind: true,
            ignore_device: false,
        }
    }
}

fn default_schema_version() -> u32 {
    1
}
fn default_kind() -> DeviceKind {
    DeviceKind::Other
}
fn default_conn() -> ConnectionType {
    ConnectionType::Dongle24G
}
fn default_enabled() -> bool {
    true
}
fn default_priority() -> i32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSpec {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub brand: Option<String>,
    pub kind: DeviceKind,
    pub connection: ConnectionType,
    pub enabled: bool,
    pub priority: i32,
    pub transport: PluginTransport,
    #[serde(rename = "match")]
    pub matcher: PluginMatchRule,
    pub protocol: Option<HidProtocolSpec>,
    pub bt_options: Option<BluetoothPluginOptions>,
}

impl Default for PluginSpec {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            id: String::new(),
            name: String::new(),
            brand: None,
            kind: default_kind(),
            connection: default_conn(),
            enabled: default_enabled(),
            priority: default_priority(),
            transport: PluginTransport::Hid,
            matcher: PluginMatchRule::default(),
            protocol: None,
            bt_options: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_hex_strings_and_integers() {
        let json = r#"{
            "id": "custom-mouse",
            "name": "Custom Mouse",
            "kind": "Mouse",
            "connection": "Dongle24G",
            "match": {
                "vid": "0x5253",
                "pids": ["1020", 4129],
                "usage_page": "FF01",
                "interface_number": 2
            },
            "protocol": {
                "mode": "feature_report",
                "request_hex": "11 06",
                "request_xor_mask": "0xFF",
                "level": {
                    "offset": 11,
                    "mask": "0xFF"
                }
            }
        }"#;
        let spec: PluginSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.matcher.vid, Some(0x5253));
        assert_eq!(spec.matcher.pids, vec![0x1020, 4129]);
        assert_eq!(spec.matcher.usage_page, Some(0xFF01));
        assert_eq!(spec.matcher.interface_number, Some(2));
        let proto = spec.protocol.unwrap();
        assert_eq!(proto.request_xor_mask, Some(0xFF));
        assert_eq!(proto.level.offset, 11);
        assert_eq!(proto.level.mask, 0xFF);
    }

    #[test]
    fn hex_bytes_and_bt_uuid_parsing() {
        assert_eq!(parse_hex_bytes("11 06 00").unwrap(), vec![0x11, 0x06, 0x00]);
        assert_eq!(
            parse_hex_bytes("0x55,0x65:01").unwrap(),
            vec![0x55, 0x65, 0x01]
        );
        assert_eq!(parse_hex_bytes("556501").unwrap(), vec![0x55, 0x65, 0x01]);
        assert_eq!(
            parse_bluetooth_uuid("180F"),
            Some(0x0000180F_0000_1000_8000_00805F9B34FB)
        );
        assert_eq!(
            parse_bluetooth_uuid("0000180f-0000-1000-8000-00805f9b34fb"),
            Some(0x0000180F_0000_1000_8000_00805F9B34FB)
        );
    }
}
