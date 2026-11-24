use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemePreference {
    System,
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LanguagePreference {
    #[default]
    Zh,
    En,
}

impl LanguagePreference {
    pub const fn is_en(self) -> bool {
        matches!(self, Self::En)
    }

    pub const fn pick<T: Copy>(self, zh: T, en: T) -> T {
        match self {
            Self::Zh => zh,
            Self::En => en,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub config_version: u32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub always_on_top: bool,
    pub compact_mode: bool,
    pub compact_max_per_row: usize,
    pub theme_mode: ThemePreference,
    pub language: LanguagePreference,
    pub poll_interval_secs: u64,
    pub show_sleeping: bool,
    pub show_phones: bool,
    pub low_battery_threshold: u8,
    pub low_battery_notify: bool,
    pub auto_start: bool,
    pub glass_backdrop: bool,
    pub hidden_device_ids: Vec<String>,
    pub custom_names: HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 2,
            window_x: None,
            window_y: None,
            always_on_top: true,
            compact_mode: false,
            compact_max_per_row: 3,
            theme_mode: ThemePreference::Dark,
            language: LanguagePreference::Zh,
            poll_interval_secs: 30,
            show_sleeping: false,
            show_phones: false,
            low_battery_threshold: 20,
            low_battery_notify: true,
            auto_start: false,
            glass_backdrop: true,
            hidden_device_ids: Vec::new(),
            custom_names: HashMap::new(),
        }
    }
}

impl AppConfig {
    pub fn app_data_dir() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("BatteryDisplay")
    }

    pub fn config_path() -> PathBuf {
        Self::app_data_dir().join("config.json")
    }

    pub fn plugins_dir() -> PathBuf {
        Self::app_data_dir().join("plugins")
    }

    pub fn crash_log_path() -> PathBuf {
        Self::app_data_dir().join("crash.log")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    pub fn load_from(path: &PathBuf) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let raw = raw.trim_start_matches('\u{feff}');
        // Detect whether `config_version` is missing/old via serde_json::Value first.
        let parsed_ver = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| v.get("config_version").and_then(|c| c.as_u64()))
            .unwrap_or(0) as u32;

        let mut cfg: Self = serde_json::from_str(raw).unwrap_or_default();
        if parsed_ver < 2 {
            cfg.show_sleeping = false;
            cfg.config_version = 2;
        }
        if cfg.compact_max_per_row == 0 {
            cfg.compact_max_per_row = 3;
        }
        cfg.compact_max_per_row = cfg.compact_max_per_row.clamp(1, 6);
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn resolve_name<'a>(&'a self, id: &str, default_name: &'a str) -> &'a str {
        self.custom_names
            .get(id)
            .map(|s| s.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(default_name)
    }
}

/// Installs a process-wide panic hook that records any unexpected panic + backtrace to
/// `%APPDATA%\BatteryDisplay\crash.log` so GUI crashes are never silent.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let bt = std::backtrace::Backtrace::force_capture();
        let log_path = AppConfig::crash_log_path();
        if let Some(parent) = log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(
                f,
                "\n[{}] PANIC on thread '{}' at {}: {}\nBacktrace:\n{}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                thread_name,
                location,
                payload,
                bt
            );
        }
        prev(info);
    }));
}

#[cfg(target_os = "windows")]
pub fn sync_autostart_registry(enabled: bool) -> Result<()> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::{PCWSTR, w};

    let subkey = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let value_name = w!("BatteryDisplay");

    unsafe {
        let mut hkey = Default::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey,
            Some(0),
            KEY_SET_VALUE,
            &mut hkey,
        );
        if status.is_err() {
            use windows::Win32::System::Registry::RegCreateKeyExW;
            let create_status = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey,
                Some(0),
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            );
            if create_status.is_err() {
                anyhow::bail!("RegCreateKeyExW failed: {:?}", create_status);
            }
        }

        let res = if enabled {
            let exe = std::env::current_exe()?;
            let quoted = format!("\"{}\"", exe.display());
            let mut wide: Vec<u16> = quoted.encode_utf16().collect();
            wide.push(0);
            let bytes =
                std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            let st = RegSetValueExW(hkey, value_name, Some(0), REG_SZ, Some(bytes));
            if st.is_err() {
                Err(anyhow::anyhow!("RegSetValueExW failed: {:?}", st))
            } else {
                Ok(())
            }
        } else {
            let _ = RegDeleteValueW(hkey, value_name);
            Ok(())
        };

        let _ = RegCloseKey(hkey);
        res
    }
}

#[cfg(not(target_os = "windows"))]
pub fn sync_autostart_registry(_enabled: bool) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_and_v1_migration() {
        let mut cfg = AppConfig {
            poll_interval_secs: 60,
            always_on_top: false,
            compact_mode: true,
            compact_max_per_row: 4,
            language: LanguagePreference::En,
            show_phones: true,
            ..Default::default()
        };
        cfg.custom_names
            .insert("dev-1".to_string(), "我的A7".to_string());

        let tmp = std::env::temp_dir().join("battery_display_test_cfg_v2.json");
        cfg.save_to(&tmp).unwrap();
        let loaded = AppConfig::load_from(&tmp);
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(loaded.config_version, 2);
        assert_eq!(loaded.poll_interval_secs, 60);
        assert!(!loaded.always_on_top);
        assert!(loaded.compact_mode);
        assert_eq!(loaded.compact_max_per_row, 4);
        assert_eq!(loaded.language, LanguagePreference::En);
        assert!(loaded.show_phones);
        assert_eq!(loaded.resolve_name("dev-1", "fallback"), "我的A7");
        assert_eq!(loaded.resolve_name("dev-2", "fallback"), "fallback");

        // v1 config without `config_version` (and with UTF-8 BOM) must migrate show_sleeping -> false
        let v1_json = "\u{feff}{\n            \"window_x\": 3334,\n            \"window_y\": 1645,\n            \"always_on_top\": false,\n            \"show_sleeping\": true\n        }";
        let tmp_v1 = std::env::temp_dir().join("battery_display_test_cfg_v1.json");
        fs::write(&tmp_v1, v1_json).unwrap();
        let migrated = AppConfig::load_from(&tmp_v1);
        let _ = std::fs::remove_file(&tmp_v1);

        assert_eq!(migrated.config_version, 2);
        assert!(!migrated.show_sleeping);
        assert_eq!(migrated.compact_max_per_row, 3);
        assert_eq!(migrated.language, LanguagePreference::Zh);
        assert_eq!(migrated.window_x, Some(3334));
    }
}
