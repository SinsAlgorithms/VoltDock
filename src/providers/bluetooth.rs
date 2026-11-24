use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::Local;

use crate::model::{ConnectionType, DeviceKind, DeviceReading, PowerState};
use crate::plugins::runner::{LoadedPlugin, matches_bt_rule};
use crate::plugins::schema::{PluginTransport, parse_bluetooth_uuid};
use crate::providers::{BatteryProvider, ProviderContext};

#[derive(Debug, Clone)]
pub struct BluetoothScanInfo {
    pub mac: String,
    pub name: String,
    pub is_le: bool,
    pub has_classic: bool,
    pub connected: bool,
    pub kind: DeviceKind,
    pub connection: ConnectionType,
    pub battery_percent: Option<u8>,
}

pub struct BluetoothProvider;

/// Extracts a 12-hex-digit Bluetooth MAC address from a Windows PnP InstanceId.
pub fn extract_bt_mac(instance_id: &str) -> Option<String> {
    let upper = instance_id.to_ascii_uppercase();
    if let Some(idx) = upper.find("DEV_") {
        let rest = &upper[idx + 4..];
        let cand: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if cand.len() == 12 {
            return Some(cand);
        }
    }
    if let Some(idx) = upper.find("BLUETOOTHDEVICE_") {
        let rest = &upper[idx + 16..];
        let cand: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if cand.len() == 12 {
            return Some(cand);
        }
    }
    // Look for `_XXXXXXXXXXXX_` or `_XXXXXXXXXXXX\` or `&XXXXXXXXXXXX`
    for seg in upper.split(['\\', '&', '_']) {
        if seg.len() == 12 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
            // Avoid matching all-zero or VID/PID style segments unless valid MAC
            if seg != "000000000000" {
                return Some(seg.to_string());
            }
        }
    }
    None
}

pub fn clean_bt_name(raw: &str) -> String {
    let mut name = raw.trim().to_string();
    for suffix in [
        " Hands-Free AG Audio",
        " Hands-Free HF Audio",
        " Hands-Free AG",
        " Hands-Free HF",
        " Hands-Free",
        " Avrcp Transport",
        " Stereo",
    ] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim().to_string();
        }
    }
    name
}

pub fn infer_bt_kind(
    name: &str,
    has_hf_phone: bool,
    has_audio_ag: bool,
    cod_raw: Option<u32>,
    appearance_raw: Option<u16>,
) -> DeviceKind {
    if has_hf_phone {
        return DeviceKind::Phone;
    }

    let lower = name.to_ascii_lowercase();
    if lower.contains("mouse") || lower.contains("鼠标") {
        return DeviceKind::Mouse;
    }
    if lower.contains("gamepad")
        || lower.contains("controller")
        || lower.contains("xbox")
        || lower.contains("dualsense")
        || lower.contains("dualshock")
        || lower.contains("joy-con")
        || lower.contains("pro controller")
        || lower.contains("gamesir")
        || lower.contains("8bitdo")
        || lower.contains("flydigi")
        || lower.contains("手柄")
    {
        return DeviceKind::Gamepad;
    }
    if lower.contains("keydial")
        || lower.contains("keyboard")
        || lower.contains("keychron")
        || lower.contains("键盘")
    {
        return DeviceKind::Keyboard;
    }
    if lower.contains("buds")
        || lower.contains("pods")
        || lower.contains("pill")
        || lower.contains("tws")
        || lower.contains("earbud")
        || lower.contains("airpods")
    {
        return DeviceKind::Earbuds;
    }
    if lower.contains("headset")
        || lower.contains("headphone")
        || lower.contains("wh-")
        || lower.contains("qc")
        || lower.contains("w820")
        || lower.contains("耳机")
    {
        return DeviceKind::Headset;
    }

    if let Some(app) = appearance_raw {
        let cat = app >> 6;
        let sub = app & 0x3F;
        // HID appearance category = 0x0F (15)
        if cat == 15 {
            return match sub {
                1 => DeviceKind::Keyboard,
                2 => DeviceKind::Mouse,
                3 | 4 => DeviceKind::Gamepad,
                _ => DeviceKind::Other,
            };
        }
        if cat == 1 {
            return DeviceKind::Phone;
        }
    }

    if let Some(cod) = cod_raw {
        let major = (cod >> 8) & 0x1F;
        let minor = (cod >> 2) & 0x3F;
        match major {
            0x02 => return DeviceKind::Phone,
            0x04 => {
                // Audio/Video
                return match minor {
                    0x01 => DeviceKind::Earbuds,
                    0x02 | 0x06 => DeviceKind::Headset,
                    _ => DeviceKind::Headset,
                };
            }
            0x05 => {
                // Peripheral
                let sub = (cod >> 6) & 0x03;
                let low = (cod >> 2) & 0x0F;
                if sub == 1 {
                    return DeviceKind::Keyboard;
                }
                if sub == 2 {
                    return DeviceKind::Mouse;
                }
                if low == 1 || low == 2 {
                    return DeviceKind::Gamepad;
                }
            }
            _ => {}
        }
    }

    if has_audio_ag {
        return DeviceKind::Headset;
    }
    DeviceKind::Other
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Devices::Bluetooth::{
        BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice,
        GenericAttributeProfile::GattCommunicationStatus,
    };
    use windows::Devices::Bluetooth::GenericAttributeProfile::GattDeviceService;
    use windows::Storage::Streams::DataReader;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
        SetupDiGetDevicePropertyW,
    };
    use windows::Win32::Devices::Properties::{
        DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_STRING,
        DEVPKEY_Device_FriendlyName, DEVPKEY_NAME, DEVPROPTYPE,
    };
    use windows::Win32::Foundation::{DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, GetLastError};
    use windows::core::{GUID, PCWSTR};
    use windows_future::{AsyncStatus, IAsyncOperation};

    const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
        pid: 2,
    };
    const DEVPKEY_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29),
        pid: 15,
    };
    const GATT_BATTERY_SERVICE_UUID: GUID =
        GUID::from_u128(0x0000180F_0000_1000_8000_00805F9B34FB);
    const GATT_BATTERY_LEVEL_UUID: GUID =
        GUID::from_u128(0x00002A19_0000_1000_8000_00805F9B34FB);

    #[derive(Default)]
    struct BtGroup {
        is_le: bool,
        has_classic: bool,
        has_hf_phone: bool,
        has_audio_ag: bool,
        pnp_battery: Option<u8>,
        pnp_connected: Option<bool>,
        root_name: Option<String>,
        best_name: Option<String>,
    }

    fn utf16_slice_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        OsString::from_wide(&buf[..end])
            .to_string_lossy()
            .trim()
            .to_string()
    }

    unsafe fn read_prop_string(
        hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
        devinfo: &SP_DEVINFO_DATA,
        key: &DEVPROPKEY,
    ) -> Option<String> {
        let mut ptype = DEVPROPTYPE::default();
        let mut req = 0u32;
        unsafe {
            let _ = SetupDiGetDevicePropertyW(
                hdev,
                devinfo,
                key,
                &mut ptype,
                None,
                Some(&mut req),
                0,
            );
            if req < 2 || GetLastError() != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
            let mut buf = vec![0u8; req as usize];
            if SetupDiGetDevicePropertyW(
                hdev,
                devinfo,
                key,
                &mut ptype,
                Some(&mut buf),
                None,
                0,
            )
            .is_err()
            {
                return None;
            }
            if ptype != DEVPROP_TYPE_STRING {
                return None;
            }
            let wlen = buf.len() / 2;
            let wslice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, wlen);
            let s = utf16_slice_to_string(wslice);
            if s.is_empty() { None } else { Some(s) }
        }
    }

    unsafe fn read_prop_byte(
        hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
        devinfo: &SP_DEVINFO_DATA,
        key: &DEVPROPKEY,
    ) -> Option<u8> {
        let mut ptype = DEVPROPTYPE::default();
        let mut buf = [0u8; 4];
        let mut req = 0u32;
        unsafe {
            if SetupDiGetDevicePropertyW(
                hdev,
                devinfo,
                key,
                &mut ptype,
                Some(&mut buf),
                Some(&mut req),
                0,
            )
            .is_err()
            {
                return None;
            }
        }
        if ptype == DEVPROP_TYPE_BYTE && req >= 1 {
            Some(buf[0])
        } else {
            None
        }
    }

    unsafe fn read_prop_bool(
        hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
        devinfo: &SP_DEVINFO_DATA,
        key: &DEVPROPKEY,
    ) -> Option<bool> {
        let mut ptype = DEVPROPTYPE::default();
        let mut buf = [0u8; 4];
        let mut req = 0u32;
        unsafe {
            if SetupDiGetDevicePropertyW(
                hdev,
                devinfo,
                key,
                &mut ptype,
                Some(&mut buf),
                Some(&mut req),
                0,
            )
            .is_err()
            {
                return None;
            }
        }
        if ptype == DEVPROP_TYPE_BOOLEAN && req >= 1 {
            Some(buf[0] != 0)
        } else {
            None
        }
    }

    fn wait_async<T: windows::core::RuntimeType>(
        op: &IAsyncOperation<T>,
        timeout_ms: u64,
    ) -> Option<T> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            match op.Status() {
                Ok(AsyncStatus::Completed) => return op.GetResults().ok(),
                Ok(AsyncStatus::Error) | Ok(AsyncStatus::Canceled) | Err(_) => return None,
                Ok(AsyncStatus::Started) => {
                    if Instant::now() >= deadline {
                        let _ = op.Cancel();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(12));
                }
                Ok(_) => {
                    if Instant::now() >= deadline {
                        let _ = op.Cancel();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(12));
                }
            }
        }
    }

    fn read_le_gatt_battery_uuid(
        le_dev: &BluetoothLEDevice,
        svc_uuid: GUID,
        char_uuid: GUID,
        offset: usize,
        scale_max: u16,
    ) -> Option<u8> {
        let svc_op = le_dev.GetGattServicesForUuidAsync(svc_uuid).ok()?;
        let svc_res = wait_async(&svc_op, 1200)?;
        if svc_res.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let services = svc_res.Services().ok()?;
        if services.Size().ok()? == 0 {
            return None;
        }
        let service: GattDeviceService = services.GetAt(0).ok()?;
        let char_op = service.GetCharacteristicsForUuidAsync(char_uuid).ok()?;
        let char_res = wait_async(&char_op, 1000)?;
        if char_res.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let chars = char_res.Characteristics().ok()?;
        if chars.Size().ok()? == 0 {
            return None;
        }
        let ch = chars.GetAt(0).ok()?;
        let read_op = ch.ReadValueAsync().ok()?;
        let read_res = wait_async(&read_op, 1000)?;
        if read_res.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let buf = read_res.Value().ok()?;
        let len = buf.Length().ok()? as usize;
        if len <= offset {
            return None;
        }
        let reader = DataReader::FromBuffer(&buf).ok()?;
        let mut raw = 0u8;
        for _ in 0..=offset {
            raw = reader.ReadByte().ok()?;
        }
        if scale_max > 0 && scale_max != 100 {
            if (raw as u16) > scale_max {
                return None;
            }
            Some(
                ((raw as f64 / scale_max as f64) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as u8,
            )
        } else if raw <= 100 {
            Some(raw)
        } else {
            None
        }
    }

    fn read_le_gatt_battery(le_dev: &BluetoothLEDevice) -> Option<u8> {
        read_le_gatt_battery_uuid(
            le_dev,
            GATT_BATTERY_SERVICE_UUID,
            GATT_BATTERY_LEVEL_UUID,
            0,
            100,
        )
    }

    pub fn poll_or_scan_bluetooth(
        plugins: &[LoadedPlugin],
        include_scan_list: bool,
        diag: &mut Vec<String>,
    ) -> (Vec<DeviceReading>, Vec<BluetoothScanInfo>) {
        let mut groups: BTreeMap<String, BtGroup> = BTreeMap::new();

        unsafe {
            let hdev = match SetupDiGetClassDevsW(
                None,
                PCWSTR::null(),
                None,
                DIGCF_ALLCLASSES | DIGCF_PRESENT,
            ) {
                Ok(h) => h,
                Err(e) => {
                    diag.push(format!("[bluetooth] SetupDiGetClassDevsW failed: {e}"));
                    return (Vec::new(), Vec::new());
                }
            };

            let mut idx = 0u32;
            let mut id_buf = vec![0u16; 512];
            loop {
                let mut devinfo = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(hdev, idx, &mut devinfo).is_err() {
                    break;
                }
                idx += 1;

                let mut req = 0u32;
                if SetupDiGetDeviceInstanceIdW(
                    hdev,
                    &devinfo,
                    Some(&mut id_buf),
                    Some(&mut req),
                )
                .is_err()
                {
                    continue;
                }
                let inst = utf16_slice_to_string(&id_buf);
                let upper = inst.to_ascii_uppercase();
                if !(upper.starts_with("BTHENUM\\")
                    || upper.starts_with("BTHLE\\")
                    || upper.starts_with("BTHLEDEVICE\\")
                    || upper.starts_with("BTHHFENUM\\"))
                {
                    continue;
                }

                let Some(mac) = extract_bt_mac(&inst) else {
                    continue;
                };

                let entry = groups.entry(mac.clone()).or_default();

                let is_root =
                    upper.starts_with("BTHENUM\\DEV_") || upper.starts_with("BTHLE\\DEV_");
                if upper.starts_with("BTHLE") {
                    entry.is_le = true;
                } else if upper.starts_with("BTHENUM\\") {
                    entry.has_classic = true;
                }
                if upper.contains("0000111F") {
                    entry.has_hf_phone = true;
                }
                if upper.contains("0000111E") || upper.contains("0000110B") {
                    entry.has_audio_ag = true;
                }

                if let Some(bat) = read_prop_byte(hdev, &devinfo, &DEVPKEY_BLUETOOTH_BATTERY)
                    && bat <= 100
                {
                    entry.pnp_battery = Some(bat);
                }

                if is_root && let Some(conn) = read_prop_bool(hdev, &devinfo, &DEVPKEY_IS_CONNECTED)
                {
                    entry.pnp_connected = Some(conn);
                }

                let fname = read_prop_string(hdev, &devinfo, &DEVPKEY_Device_FriendlyName)
                    .or_else(|| read_prop_string(hdev, &devinfo, &DEVPKEY_NAME));
                if let Some(n) = fname {
                    let cleaned = clean_bt_name(&n);
                    if !cleaned.is_empty() {
                        if is_root {
                            entry.root_name = Some(cleaned.clone());
                        }
                        let generic = cleaned.starts_with("Bluetooth")
                            || cleaned.starts_with("Generic ")
                            || cleaned.starts_with("Headset Audio Gateway")
                            || cleaned.starts_with("Object Push")
                            || cleaned.starts_with("Personal Area")
                            || cleaned.starts_with("Phonebook Access")
                            || cleaned.starts_with("Device Information");
                        if !generic && entry.best_name.is_none() {
                            entry.best_name = Some(cleaned);
                        }
                    }
                }
            }

            let _ = SetupDiDestroyDeviceInfoList(hdev);
        }

        let mut readings = Vec::new();
        let mut scan_items = Vec::new();

        for (mac, g) in groups {
            let Ok(addr) = u64::from_str_radix(&mac, 16) else {
                continue;
            };

            let mut connected = false;
            let mut winrt_name: Option<String> = None;
            let mut cod_raw: Option<u32> = None;
            let mut app_raw: Option<u16> = None;
            let mut battery = g.pnp_battery;
            let mut le_dev_handle: Option<BluetoothLEDevice> = None;

            if g.is_le
                && let Ok(op) = BluetoothLEDevice::FromBluetoothAddressAsync(addr)
                && let Some(le) = wait_async(&op, 1200)
            {
                if let Ok(st) = le.ConnectionStatus() {
                    connected = st == BluetoothConnectionStatus::Connected;
                }
                if let Ok(n) = le.Name() {
                    let s = n.to_string_lossy();
                    if !s.trim().is_empty() {
                        winrt_name = Some(clean_bt_name(&s));
                    }
                }
                if let Ok(app) = le.Appearance() {
                    app_raw = app.RawValue().ok();
                }
                if connected && battery.is_none() {
                    battery = read_le_gatt_battery(&le);
                }
                le_dev_handle = Some(le);
            }

            if !connected
                && g.has_classic
                && let Ok(op) = BluetoothDevice::FromBluetoothAddressAsync(addr)
                && let Some(bt) = wait_async(&op, 1200)
            {
                if let Ok(st) = bt.ConnectionStatus() {
                    connected = st == BluetoothConnectionStatus::Connected;
                }
                if let Ok(n) = bt.Name() {
                    let s = n.to_string_lossy();
                    if !s.trim().is_empty() {
                        winrt_name = Some(clean_bt_name(&s));
                    }
                }
                if let Ok(cod) = bt.ClassOfDevice() {
                    cod_raw = cod.RawValue().ok();
                }
            }

            if !connected && let Some(pnp_c) = g.pnp_connected {
                connected = pnp_c;
            }

            let mut display_name = g
                .root_name
                .or(winrt_name)
                .or(g.best_name)
                .unwrap_or_else(|| format!("蓝牙设备 {mac}"));

            let mut kind = infer_bt_kind(
                &display_name,
                g.has_hf_phone,
                g.has_audio_ag,
                cod_raw,
                app_raw,
            );
            let conn_type = if g.is_le {
                ConnectionType::BluetoothLE
            } else {
                ConnectionType::Bluetooth
            };

            let matched_plugin = plugins.iter().find(|p| {
                p.spec.enabled
                    && p.spec.transport == PluginTransport::Bluetooth
                    && matches_bt_rule(&display_name, &mac, &p.spec.matcher)
            });

            let mut brand: Option<String> = None;
            let mut provider_name = "bluetooth".to_string();
            let mut detail: Option<String> = None;
            let mut ignored = false;

            if let Some(loaded) = matched_plugin {
                let spec = &loaded.spec;
                provider_name = format!("plugin:{}", spec.id);
                detail = Some(format!("插件: {}", spec.id));
                if !spec.name.trim().is_empty() {
                    display_name = spec.name.clone();
                }
                brand = spec.brand.clone();
                if let Some(opts) = &spec.bt_options {
                    if opts.ignore_device {
                        ignored = true;
                    }
                    if opts.override_kind {
                        kind = spec.kind;
                    }
                    if !ignored
                        && connected
                        && battery.is_none()
                        && let Some(ref le) = le_dev_handle
                        && let (Some(svc_s), Some(chr_s)) =
                            (&opts.gatt_service_uuid, &opts.gatt_char_uuid)
                        && let (Some(svc_u), Some(chr_u)) =
                            (parse_bluetooth_uuid(svc_s), parse_bluetooth_uuid(chr_s))
                    {
                        battery = read_le_gatt_battery_uuid(
                            le,
                            GUID::from_u128(svc_u),
                            GUID::from_u128(chr_u),
                            opts.level_offset,
                            opts.scale_max,
                        );
                    }
                } else {
                    kind = spec.kind;
                }
            }

            if include_scan_list {
                scan_items.push(BluetoothScanInfo {
                    mac: mac.clone(),
                    name: display_name.clone(),
                    is_le: g.is_le,
                    has_classic: g.has_classic,
                    connected,
                    kind,
                    connection: conn_type,
                    battery_percent: battery,
                });
            }

            diag.push(format!(
                "[bluetooth] mac={mac} name='{display_name}' le={} connected={connected} bat={battery:?} kind={kind:?} ignored={ignored}",
                g.is_le
            ));

            if ignored || !connected {
                continue;
            }
            let Some(pct) = battery else {
                continue;
            };

            readings.push(DeviceReading {
                id: format!("bt:{mac}"),
                name: display_name,
                brand,
                kind,
                connection: conn_type,
                battery_percent: Some(pct),
                power_state: if pct == 100 {
                    PowerState::Full
                } else {
                    PowerState::Discharging
                },
                provider: provider_name,
                vid_pid: None,
                mac_address: Some(mac),
                last_seen: Local::now(),
                approximate: false,
                detail,
            });
        }
        (readings, scan_items)
    }

    pub fn poll_bluetooth_with_plugins(
        plugins: &[LoadedPlugin],
        diag: &mut Vec<String>,
    ) -> Vec<DeviceReading> {
        poll_or_scan_bluetooth(plugins, false, diag).0
    }

    pub fn scan_bluetooth_devices() -> Vec<BluetoothScanInfo> {
        let mut diag = Vec::new();
        poll_or_scan_bluetooth(&[], true, &mut diag).1
    }
}

#[cfg(target_os = "windows")]
pub fn scan_bluetooth_devices() -> Vec<BluetoothScanInfo> {
    win::scan_bluetooth_devices()
}

#[cfg(not(target_os = "windows"))]
pub fn scan_bluetooth_devices() -> Vec<BluetoothScanInfo> {
    Vec::new()
}

impl BluetoothProvider {
    pub fn poll_with_plugins(
        &self,
        plugins: &[LoadedPlugin],
        diag: &mut Vec<String>,
    ) -> Vec<DeviceReading> {
        #[cfg(target_os = "windows")]
        {
            win::poll_bluetooth_with_plugins(plugins, diag)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (plugins, diag);
            Vec::new()
        }
    }
}

impl BatteryProvider for BluetoothProvider {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    fn poll(&self, _ctx: &ProviderContext<'_>, diag: &mut Vec<String>) -> Vec<DeviceReading> {
        self.poll_with_plugins(&[], diag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_extraction_across_windows_instance_ids() {
        assert_eq!(
            extract_bt_mac(r"BTHLE\DEV_202306016884\7&1EB7DD90&0&202306016884"),
            Some("202306016884".to_string())
        );
        assert_eq!(
            extract_bt_mac(
                r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\7&8029428&3&250213B6C96A_C00000000"
            ),
            Some("250213B6C96A".to_string())
        );
        assert_eq!(
            extract_bt_mac(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_A4C3BEE9722C\8&37E366EB&0&009A"),
            Some("A4C3BEE9722C".to_string())
        );
    }

    #[test]
    fn name_cleanup_and_kind_classification() {
        assert_eq!(clean_bt_name("MOONDROP PILL Hands-Free AG"), "MOONDROP PILL");
        assert_eq!(
            infer_bt_kind("C.X.K.的REDMI K80", true, false, None, None),
            DeviceKind::Phone
        );
        assert_eq!(
            infer_bt_kind("MOONDROP PILL", false, true, None, None),
            DeviceKind::Earbuds
        );
        assert_eq!(
            infer_bt_kind("EDIFIER W820NB", false, true, None, None),
            DeviceKind::Headset
        );
        assert_eq!(
            infer_bt_kind("Keydial mini-756", false, false, None, None),
            DeviceKind::Keyboard
        );
    }
}
