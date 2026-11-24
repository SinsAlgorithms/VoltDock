pub mod bluetooth;
pub mod brands_24g;
pub mod gamepad_std;
pub mod mchose_headset;
pub mod mchose_mouse;
pub mod gamesir;

use crate::model::DeviceReading;
use crate::transport::hid::HidDeviceInfo;

pub struct ProviderContext<'a> {
    pub hid_devices: &'a [HidDeviceInfo],
}

pub trait BatteryProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn poll(&self, ctx: &ProviderContext<'_>, diagnostics: &mut Vec<String>) -> Vec<DeviceReading>;
}
