use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Local};

use crate::config::LanguagePreference;
use crate::model::{DeviceKind, DeviceReading, PollReport, PowerState};
use crate::plugins::runner::{
    LoadedPlugin, PluginHidProvider, PluginLoadError, load_all_plugins,
};
use crate::providers::bluetooth::BluetoothProvider;
use crate::providers::brands_24g::Brands24GProvider;
use crate::providers::gamepad_std::StandardGamepadProvider;
use crate::providers::gamesir::GameSirProvider;
use crate::providers::mchose_headset::MchoseHeadsetProvider;
use crate::providers::mchose_mouse::MchoseMouseProvider;
use crate::providers::{BatteryProvider, ProviderContext};
use crate::transport::hid::enumerate_hid_devices;

#[derive(Debug, Clone)]
pub enum PollCommand {
    PollNow,
    ReloadPlugins,
    SetIntervalSecs(u64),
    Shutdown,
}

#[derive(Debug, Clone)]
struct CachedBattery {
    percent: u8,
    seen_at: DateTime<Local>,
}

#[derive(Default)]
pub struct PollStateTracker {
    last_known_battery: HashMap<String, CachedBattery>,
    low_alerted: HashSet<String>,
}

impl PollStateTracker {
    /// Applies sleep-cache retention (preserving last known % when a wireless dongle
    /// reports Sleeping or Charging without an exact percentage like GameSir 0xFF)
    /// and merges/deduplicates multi-transport reports.
    pub fn process(&mut self, mut raw: Vec<DeviceReading>) -> Vec<DeviceReading> {
        let now = Local::now();

        // Update cache for any live numeric battery reading
        for dev in &raw {
            if let Some(pct) = dev.battery_percent
                && !dev.power_state.is_inactive()
            {
                self.last_known_battery.insert(
                    dev.id.clone(),
                    CachedBattery {
                        percent: pct,
                        seen_at: now,
                    },
                );
            }
        }

        // Promote Offline -> Sleeping for any device that recently had a live reading (within 30 min).
        // Devices never seen connected stay Offline (always hidden). Backfill missing % for active/sleeping.
        for dev in &mut raw {
            if dev.power_state == PowerState::Offline {
                if let Some(cached) = self.last_known_battery.get(&dev.id)
                    && (now - cached.seen_at).num_seconds() < 1800
                {
                    dev.power_state = PowerState::Sleeping;
                    dev.battery_percent = Some(cached.percent);
                    dev.detail = Some("设备短暂休眠（显示最近电量）".to_string());
                }
                continue;
            }
            if dev.battery_percent.is_none()
                && let Some(cached) = self.last_known_battery.get(&dev.id)
                && (now - cached.seen_at).num_seconds() < 12 * 3600
            {
                dev.battery_percent = Some(cached.percent);
            }
        }

        deduplicate_readings(raw)
    }

    /// Returns newly triggered low-battery notifications with hysteresis (`threshold + 5` reset).
    pub fn collect_low_battery_alerts(
        &mut self,
        devices: &[DeviceReading],
        threshold: u8,
        enabled: bool,
    ) -> Vec<DeviceReading> {
        if !enabled {
            return Vec::new();
        }
        let mut alerts = Vec::new();
        for d in devices {
            if d.power_state.is_inactive() {
                continue;
            }
            if d.power_state.is_charging_or_full() {
                self.low_alerted.remove(&d.id);
                continue;
            }
            if let Some(pct) = d.battery_percent {
                if pct <= threshold {
                    if self.low_alerted.insert(d.id.clone()) {
                        alerts.push(d.clone());
                    }
                } else if pct > threshold.saturating_add(5) {
                    self.low_alerted.remove(&d.id);
                }
            }
        }
        alerts
    }
}

pub fn deduplicate_readings(raw: Vec<DeviceReading>) -> Vec<DeviceReading> {
    let active_plugin_vps: HashSet<(u16, u16)> = raw
        .iter()
        .filter(|d| d.provider.starts_with("plugin:") && d.power_state != PowerState::Offline)
        .filter_map(|d| d.vid_pid)
        .collect();

    // 1. Check if a vendor gamepad provider (GameSir / FlyDigi / Sony / Switch / 8BitDo or plugin) is active
    let has_active_vendor_dongle_pad = raw.iter().any(|d| {
        d.kind == DeviceKind::Gamepad
            && (matches!(d.provider.as_str(), "gamesir_hid" | "flydigi_hid")
                || d.provider.starts_with("plugin:"))
            && !d.power_state.is_inactive()
    });
    let has_any_vendor_pad = raw.iter().any(|d| {
        d.kind == DeviceKind::Gamepad
            && matches!(
                d.provider.as_str(),
                "gamesir_hid" | "flydigi_hid" | "playstation_hid" | "nintendo_hid" | "8bitdo_hid"
            )
    });
    let has_bt_gamepad = raw
        .iter()
        .any(|d| d.kind == DeviceKind::Gamepad && d.connection.is_bluetooth());

    let mut by_id: HashMap<String, DeviceReading> = HashMap::new();
    for d in raw {
        // User plugins that actively produce a reading override built-in providers for the same VID:PID
        if !d.provider.starts_with("plugin:")
            && let Some(vp) = d.vid_pid
            && active_plugin_vps.contains(&vp)
        {
            continue;
        }

        // Suppress redundant generic XInput / WGI entry when a vendor protocol owns that gamepad
        if d.provider == "xinput" {
            if has_active_vendor_dongle_pad {
                continue;
            }
            if has_any_vendor_pad && d.battery_percent.is_none() {
                continue;
            }
            if has_bt_gamepad {
                continue;
            }
        }
        if d.provider == "wgi" && (has_active_vendor_dongle_pad || has_bt_gamepad) {
            continue;
        }

        // Merge same device ID keeping the higher-fidelity / active / plugin / charging entry
        match by_id.get(&d.id) {
            Some(existing) => {
                if score_reading(&d) > score_reading(existing) {
                    by_id.insert(d.id.clone(), d);
                }
            }
            None => {
                by_id.insert(d.id.clone(), d);
            }
        }
    }

    let mut out: Vec<DeviceReading> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        a.kind
            .sort_rank()
            .cmp(&b.kind.sort_rank())
            .then_with(|| a.power_state.is_inactive().cmp(&b.power_state.is_inactive()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn score_reading(d: &DeviceReading) -> (u8, u8, u8, u8, u8) {
    let active = u8::from(!d.power_state.is_inactive());
    let has_pct = u8::from(d.battery_percent.is_some());
    let is_plugin = u8::from(d.provider.starts_with("plugin:"));
    let charging = u8::from(d.power_state.is_charging_or_full());
    let exact = u8::from(!d.approximate);
    (active, has_pct, is_plugin, charging, exact)
}

pub fn run_single_poll() -> PollReport {
    #[cfg(target_os = "windows")]
    let _com_guard = ComMtaGuard::enter();

    let (plugins, plugin_errors) = load_all_plugins();
    let mut report = collect_raw_poll(Arc::new(plugins), &plugin_errors);
    report.devices = deduplicate_readings(report.devices);
    report
}

pub fn run_test_single_plugin(plugin: LoadedPlugin) -> PollReport {
    #[cfg(target_os = "windows")]
    let _com_guard = ComMtaGuard::enter();

    let hid_devices = enumerate_hid_devices();
    let ctx = ProviderContext {
        hid_devices: &hid_devices,
    };
    let mut diagnostics = vec![format!(
        "[test-plugin] testing '{}' (transport={:?}) against {} HID interfaces",
        plugin.spec.id,
        plugin.spec.transport,
        hid_devices.len()
    )];
    let plugins_arc = Arc::new(vec![plugin]);
    let mut devices = PluginHidProvider {
        plugins: Arc::clone(&plugins_arc),
    }
    .poll(&ctx, &mut diagnostics);
    let bt_devs = BluetoothProvider.poll_with_plugins(&plugins_arc, &mut diagnostics);
    devices.extend(bt_devs);
    PollReport {
        devices: deduplicate_readings(devices),
        diagnostics,
        polled_at: Local::now(),
    }
}

pub fn spawn_poll_worker(
    initial_interval_secs: u64,
) -> (mpsc::Sender<PollCommand>, smol::channel::Receiver<PollReport>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<PollCommand>();
    let (report_tx, report_rx) = smol::channel::unbounded::<PollReport>();

    thread::Builder::new()
        .name("battery-poller".to_string())
        .spawn(move || {
            #[cfg(target_os = "windows")]
            let _com_guard = ComMtaGuard::enter();

            let mut tracker = PollStateTracker::default();
            let mut interval = Duration::from_secs(initial_interval_secs.max(5));
            let (init_plugins, mut plugin_errors) = load_all_plugins();
            let mut plugins_arc = Arc::new(init_plugins);

            loop {
                let mut report = collect_raw_poll(Arc::clone(&plugins_arc), &plugin_errors);
                report.devices = tracker.process(report.devices);
                if report_tx.send_blocking(report).is_err() {
                    break;
                }

                // Wait up to interval for commands; coalesce rapid PollNow/ReloadPlugins requests
                match cmd_rx.recv_timeout(interval) {
                    Ok(first) => {
                        let mut should_poll_now = matches!(
                            first,
                            PollCommand::PollNow | PollCommand::ReloadPlugins
                        );
                        let mut handle_cmd = |cmd: PollCommand| -> bool {
                            match cmd {
                                PollCommand::PollNow => {
                                    should_poll_now = true;
                                    true
                                }
                                PollCommand::ReloadPlugins => {
                                    let (p, errs) = load_all_plugins();
                                    plugins_arc = Arc::new(p);
                                    plugin_errors = errs;
                                    should_poll_now = true;
                                    true
                                }
                                PollCommand::SetIntervalSecs(s) => {
                                    interval = Duration::from_secs(s.max(5));
                                    true
                                }
                                PollCommand::Shutdown => false,
                            }
                        };
                        if !handle_cmd(first) {
                            break;
                        }
                        while let Ok(cmd) = cmd_rx.try_recv() {
                            if !handle_cmd(cmd) {
                                return;
                            }
                        }
                        if !should_poll_now {
                            continue;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("failed to spawn battery poller thread");

    (cmd_tx, report_rx)
}

fn collect_raw_poll(
    plugins: Arc<Vec<LoadedPlugin>>,
    plugin_errors: &[PluginLoadError],
) -> PollReport {
    let hid_devices = enumerate_hid_devices();
    let ctx = ProviderContext {
        hid_devices: &hid_devices,
    };

    let mut diagnostics = Vec::new();
    diagnostics.push(format!(
        "[system] enumerated {} HID interfaces, {} active plugins at {}",
        hid_devices.len(),
        plugins.iter().filter(|p| p.spec.enabled).count(),
        Local::now().format("%H:%M:%S")
    ));
    for err in plugin_errors {
        diagnostics.push(format!(
            "[plugin-err] {}: {}",
            err.path.display(),
            err.error
        ));
    }

    let providers: Vec<Box<dyn BatteryProvider>> = vec![
        Box::new(PluginHidProvider {
            plugins: Arc::clone(&plugins),
        }),
        Box::new(MchoseMouseProvider),
        Box::new(MchoseHeadsetProvider),
        Box::new(GameSirProvider),
        Box::new(StandardGamepadProvider),
        Box::new(Brands24GProvider),
    ];

    let mut raw = Vec::new();
    for provider in &providers {
        let mut found = provider.poll(&ctx, &mut diagnostics);
        raw.append(&mut found);
    }
    let mut bt_found = BluetoothProvider.poll_with_plugins(&plugins, &mut diagnostics);
    raw.append(&mut bt_found);

    PollReport {
        devices: raw,
        diagnostics,
        polled_at: Local::now(),
    }
}

#[cfg(target_os = "windows")]
struct ComMtaGuard(bool);

#[cfg(target_os = "windows")]
impl ComMtaGuard {
    fn enter() -> Self {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self(hr.is_ok())
    }
}

#[cfg(target_os = "windows")]
impl Drop for ComMtaGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// Filters visible devices according to connection state and user configuration.
/// Devices with `PowerState::Offline` (unconnected) are always hidden.
pub fn filter_visible_devices<'a>(
    devices: &'a [DeviceReading],
    show_sleeping: bool,
    show_phones: bool,
    hidden_ids: &[String],
) -> Vec<&'a DeviceReading> {
    devices
        .iter()
        .filter(|d| {
            if hidden_ids.iter().any(|id| id == &d.id) {
                return false;
            }
            if !show_phones && d.kind == DeviceKind::Phone {
                return false;
            }
            if d.power_state == PowerState::Offline {
                return false;
            }
            if d.power_state == PowerState::Sleeping
                && (!show_sleeping || d.battery_percent.is_none())
            {
                return false;
            }
            true
        })
        .collect()
}

/// Formats a concise tooltip summary for the tray icon.
pub fn format_tray_tooltip(devices: &[DeviceReading], lang: LanguagePreference) -> String {
    let visible: Vec<&DeviceReading> = devices
        .iter()
        .filter(|d| d.kind != DeviceKind::Phone && !d.power_state.is_inactive())
        .take(3)
        .collect();
    if visible.is_empty() {
        return lang
            .pick(
                "设备电量小组件 - 点击显示/隐藏",
                "VoltDock - Click to show/hide",
            )
            .to_string();
    }
    let parts: Vec<String> = visible
        .iter()
        .map(|d| format!("{}: {}", d.name, d.display_percent()))
        .collect();
    let joined = parts.join(" | ");
    // NOTIFYICONDATAW szTip max is 128 UTF-16 code units
    joined.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConnectionType;

    fn mk_dev(
        id: &str,
        kind: DeviceKind,
        provider: &str,
        pct: Option<u8>,
        state: PowerState,
    ) -> DeviceReading {
        DeviceReading {
            id: id.to_string(),
            name: id.to_string(),
            brand: None,
            kind,
            connection: ConnectionType::Dongle24G,
            battery_percent: pct,
            power_state: state,
            provider: provider.to_string(),
            vid_pid: None,
            mac_address: None,
            last_seen: Local::now(),
            approximate: false,
            detail: None,
        }
    }

    #[test]
    fn sleep_retention_offline_and_xinput_dedup() {
        let mut tracker = PollStateTracker::default();

        // Poll 1: active mouse 79% + active GameSir 85% + duplicate XInput wired + never-connected Offline headset
        let r1 = tracker.process(vec![
            mk_dev("mchose:mouse", DeviceKind::Mouse, "mchose_mouse", Some(79), PowerState::Discharging),
            mk_dev("gamesir:2106", DeviceKind::Gamepad, "gamesir_hid", Some(85), PowerState::Discharging),
            mk_dev("xinput:0", DeviceKind::Gamepad, "xinput", None, PowerState::Charging),
            mk_dev("mchose:v9_pro", DeviceKind::Headset, "mchose_headset", None, PowerState::Offline),
        ]);
        assert_eq!(r1.len(), 3);
        assert!(r1.iter().all(|d| d.provider != "xinput"));
        let v9 = r1.iter().find(|d| d.id == "mchose:v9_pro").unwrap();
        assert_eq!(v9.power_state, PowerState::Offline);
        assert_eq!(v9.battery_percent, None);

        // Poll 2: both recently active mouse & gamepad fall idle -> promoted to Sleeping with cached %;
        // never-connected headset stays Offline (no cache).
        let r2 = tracker.process(vec![
            mk_dev("mchose:mouse", DeviceKind::Mouse, "mchose_mouse", None, PowerState::Offline),
            mk_dev("gamesir:2106", DeviceKind::Gamepad, "gamesir_hid", None, PowerState::Offline),
            mk_dev("mchose:v9_pro", DeviceKind::Headset, "mchose_headset", None, PowerState::Offline),
        ]);
        let mouse = r2.iter().find(|d| d.id == "mchose:mouse").unwrap();
        let pad = r2.iter().find(|d| d.id == "gamesir:2106").unwrap();
        assert_eq!(mouse.battery_percent, Some(79));
        assert_eq!(mouse.power_state, PowerState::Sleeping);
        assert_eq!(pad.battery_percent, Some(85));
        assert_eq!(pad.power_state, PowerState::Sleeping);

        // When show_sleeping == false, sleeping devices and offline devices are all hidden
        let vis_default = filter_visible_devices(&r2, false, false, &[]);
        assert!(vis_default.is_empty());

        // When show_sleeping == true, BOTH recently sleeping mouse & gamepad appear, while never-connected Offline headset stays hidden!
        let vis_with_sleep = filter_visible_devices(&r2, true, false, &[]);
        assert_eq!(vis_with_sleep.len(), 2);
        assert!(vis_with_sleep.iter().all(|d| d.power_state == PowerState::Sleeping));

        // Explicit active device (e.g. Charging without exact %) retains cached 85% too
        let r3 = tracker.process(vec![
            mk_dev("gamesir:2106", DeviceKind::Gamepad, "gamesir_hid", None, PowerState::Charging),
        ]);
        assert_eq!(r3[0].battery_percent, Some(85));
        assert_eq!(r3[0].power_state, PowerState::Charging);
    }

    #[test]
    fn low_battery_alert_hysteresis() {
        let mut tracker = PollStateTracker::default();
        let d_low = vec![mk_dev("m1", DeviceKind::Mouse, "mchose_mouse", Some(15), PowerState::Discharging)];
        assert_eq!(tracker.collect_low_battery_alerts(&d_low, 20, true).len(), 1);
        // Should NOT alert repeatedly while still low
        assert_eq!(tracker.collect_low_battery_alerts(&d_low, 20, true).len(), 0);
        // Still within hysteresis band (22 <= 20+5) -> still suppressed
        let d_mid = vec![mk_dev("m1", DeviceKind::Mouse, "mchose_mouse", Some(22), PowerState::Discharging)];
        assert_eq!(tracker.collect_low_battery_alerts(&d_mid, 20, true).len(), 0);
        // Charging clears alert state
        let d_chg = vec![mk_dev("m1", DeviceKind::Mouse, "mchose_mouse", Some(18), PowerState::Charging)];
        assert_eq!(tracker.collect_low_battery_alerts(&d_chg, 20, true).len(), 0);
        // Dropping low again triggers fresh alert
        assert_eq!(tracker.collect_low_battery_alerts(&d_low, 20, true).len(), 1);
    }
}
