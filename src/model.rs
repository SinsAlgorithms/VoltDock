use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::config::LanguagePreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    Mouse,
    Headset,
    Gamepad,
    Earbuds,
    Keyboard,
    Other,
    Phone,
}

impl DeviceKind {
    pub fn label(self, lang: LanguagePreference) -> &'static str {
        match (self, lang) {
            (DeviceKind::Mouse, LanguagePreference::Zh) => "无线鼠标",
            (DeviceKind::Mouse, LanguagePreference::En) => "Wireless Mouse",
            (DeviceKind::Headset, LanguagePreference::Zh) => "无线耳机",
            (DeviceKind::Headset, LanguagePreference::En) => "Wireless Headset",
            (DeviceKind::Earbuds, LanguagePreference::Zh) => "真无线耳机",
            (DeviceKind::Earbuds, LanguagePreference::En) => "Wireless Earbuds",
            (DeviceKind::Gamepad, LanguagePreference::Zh) => "无线手柄",
            (DeviceKind::Gamepad, LanguagePreference::En) => "Wireless Gamepad",
            (DeviceKind::Keyboard, LanguagePreference::Zh) => "无线键盘",
            (DeviceKind::Keyboard, LanguagePreference::En) => "Wireless Keyboard",
            (DeviceKind::Phone, LanguagePreference::Zh) => "蓝牙手机",
            (DeviceKind::Phone, LanguagePreference::En) => "Bluetooth Phone",
            (DeviceKind::Other, LanguagePreference::Zh) => "无线设备",
            (DeviceKind::Other, LanguagePreference::En) => "Wireless Device",
        }
    }

    pub fn label_zh(self) -> &'static str {
        self.label(LanguagePreference::Zh)
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            DeviceKind::Mouse => "icons/device-mouse.svg",
            DeviceKind::Headset => "icons/device-headset.svg",
            DeviceKind::Earbuds => "icons/device-earbuds.svg",
            DeviceKind::Gamepad => "icons/device-gamepad.svg",
            DeviceKind::Keyboard => "icons/device-keyboard.svg",
            DeviceKind::Phone => "icons/device-phone.svg",
            DeviceKind::Other => "icons/device-generic.svg",
        }
    }

    pub fn sort_rank(self) -> u8 {
        match self {
            DeviceKind::Mouse => 0,
            DeviceKind::Headset => 1,
            DeviceKind::Gamepad => 2,
            DeviceKind::Earbuds => 3,
            DeviceKind::Keyboard => 4,
            DeviceKind::Other => 5,
            DeviceKind::Phone => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionType {
    Dongle24G,
    Bluetooth,
    BluetoothLE,
    Wired,
}

impl ConnectionType {
    pub fn badge_label_lang(self, lang: LanguagePreference) -> &'static str {
        match (self, lang) {
            (ConnectionType::Dongle24G, _) => "2.4G",
            (ConnectionType::Bluetooth, LanguagePreference::Zh) => "蓝牙",
            (ConnectionType::Bluetooth, LanguagePreference::En) => "BT",
            (ConnectionType::BluetoothLE, _) => "BLE",
            (ConnectionType::Wired, LanguagePreference::Zh) => "有线",
            (ConnectionType::Wired, LanguagePreference::En) => "Wired",
        }
    }

    pub fn badge_label(self) -> &'static str {
        self.badge_label_lang(LanguagePreference::Zh)
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            ConnectionType::Dongle24G => "icons/conn-24g.svg",
            ConnectionType::Bluetooth | ConnectionType::BluetoothLE => "icons/conn-bt.svg",
            ConnectionType::Wired => "icons/conn-usb.svg",
        }
    }

    pub fn is_bluetooth(self) -> bool {
        matches!(self, ConnectionType::Bluetooth | ConnectionType::BluetoothLE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerState {
    Discharging,
    Charging,
    Full,
    Sleeping,
    Offline,
    Unknown,
}

impl PowerState {
    pub fn label(self, lang: LanguagePreference) -> &'static str {
        match (self, lang) {
            (PowerState::Discharging, LanguagePreference::Zh) => "使用中",
            (PowerState::Discharging, LanguagePreference::En) => "Active",
            (PowerState::Charging, LanguagePreference::Zh) => "充电中",
            (PowerState::Charging, LanguagePreference::En) => "Charging",
            (PowerState::Full, LanguagePreference::Zh) => "已充满",
            (PowerState::Full, LanguagePreference::En) => "Full",
            (PowerState::Sleeping, LanguagePreference::Zh) => "休眠/待机",
            (PowerState::Sleeping, LanguagePreference::En) => "Sleeping",
            (PowerState::Offline, LanguagePreference::Zh) => "已断开",
            (PowerState::Offline, LanguagePreference::En) => "Offline",
            (PowerState::Unknown, LanguagePreference::Zh) => "未知状态",
            (PowerState::Unknown, LanguagePreference::En) => "Unknown",
        }
    }

    pub fn label_zh(self) -> &'static str {
        self.label(LanguagePreference::Zh)
    }

    pub fn is_charging_or_full(self) -> bool {
        matches!(self, PowerState::Charging | PowerState::Full)
    }

    pub fn is_inactive(self) -> bool {
        matches!(self, PowerState::Sleeping | PowerState::Offline)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceReading {
    pub id: String,
    pub name: String,
    pub brand: Option<String>,
    pub kind: DeviceKind,
    pub connection: ConnectionType,
    pub battery_percent: Option<u8>,
    pub power_state: PowerState,
    pub provider: String,
    pub vid_pid: Option<(u16, u16)>,
    pub mac_address: Option<String>,
    pub last_seen: DateTime<Local>,
    pub approximate: bool,
    pub detail: Option<String>,
}

impl DeviceReading {
    pub fn is_low_battery(&self, threshold: u8) -> bool {
        match self.battery_percent {
            Some(pct) if !self.power_state.is_charging_or_full() => pct <= threshold,
            _ => false,
        }
    }

    pub fn display_percent(&self) -> String {
        match self.battery_percent {
            Some(pct) if self.approximate => format!("~{pct}%"),
            Some(pct) => format!("{pct}%"),
            None => "--".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollReport {
    pub devices: Vec<DeviceReading>,
    pub diagnostics: Vec<String>,
    pub polled_at: DateTime<Local>,
}

impl Default for PollReport {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            diagnostics: Vec::new(),
            polled_at: Local::now(),
        }
    }
}
