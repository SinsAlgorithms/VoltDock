use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    #[allow(dead_code)]
    pub version_number: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    pub feature_report_len: u16,
    pub manufacturer: String,
    pub product: String,
    pub interface_number: Option<u8>,
}

pub fn parse_interface_number(path: &str) -> Option<u8> {
    let lower = path.to_ascii_lowercase();
    let idx = lower.find("&mi_")?;
    let hex_part = lower.get(idx + 4..idx + 6)?;
    u8::from_str_radix(hex_part, 16).ok()
}

pub fn parse_utf16_z(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end])
        .to_string_lossy()
        .trim()
        .to_string()
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ptr;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature,
        HidD_GetHidGuid, HidD_GetManufacturerString, HidD_GetPreparsedData, HidD_GetProductString,
        HidD_SetFeature, HidD_SetOutputReport, HidP_GetCaps, PHIDP_PREPARSED_DATA,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE,
        GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
    };
    use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::{GUID, PCWSTR};

    fn to_wide_null(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    pub fn enumerate_hid_devices() -> Vec<HidDeviceInfo> {
        let mut out = Vec::new();
        unsafe {
            let hid_guid: GUID = HidD_GetHidGuid();
            let dev_info = match SetupDiGetClassDevsW(
                Some(&hid_guid),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            ) {
                Ok(h) => h,
                Err(_) => return out,
            };

            let mut index = 0u32;
            loop {
                let mut if_data = SP_DEVICE_INTERFACE_DATA {
                    cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInterfaces(
                    dev_info,
                    None,
                    &hid_guid,
                    index,
                    &mut if_data,
                )
                .is_err()
                {
                    break;
                }
                index += 1;

                let mut required = 0u32;
                let _ = SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &if_data,
                    None,
                    0,
                    Some(&mut required),
                    None,
                );
                if required == 0 && GetLastError() != ERROR_INSUFFICIENT_BUFFER {
                    continue;
                }
                if required < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
                    continue;
                }

                let mut buf = vec![0u8; required as usize];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize =
                    std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

                if SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &if_data,
                    Some(detail),
                    required,
                    None,
                    None,
                )
                .is_err()
                {
                    continue;
                }

                let path_ptr = ptr::addr_of!((*detail).DevicePath) as *const u16;
                let max_chars =
                    (required as usize - std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath)) / 2;
                let path_slice = std::slice::from_raw_parts(path_ptr, max_chars);
                let path = parse_utf16_z(path_slice);
                if path.is_empty() {
                    continue;
                }

                let wide_path = to_wide_null(&path);
                // Open with access=0 to read attributes/caps without interfering with exclusive devices
                let handle = match CreateFileW(
                    PCWSTR(wide_path.as_ptr()),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) {
                    Ok(h) if !h.is_invalid() && h != INVALID_HANDLE_VALUE => h,
                    _ => continue,
                };

                let mut attrs = HIDD_ATTRIBUTES {
                    Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                    ..Default::default()
                };
                if !HidD_GetAttributes(handle, &mut attrs) {
                    let _ = CloseHandle(handle);
                    continue;
                }

                let mut mfr_buf = [0u16; 128];
                let manufacturer = if HidD_GetManufacturerString(
                    handle,
                    mfr_buf.as_mut_ptr() as *mut _,
                    (mfr_buf.len() * 2) as u32,
                ) {
                    parse_utf16_z(&mfr_buf)
                } else {
                    String::new()
                };

                let mut prod_buf = [0u16; 128];
                let product = if HidD_GetProductString(
                    handle,
                    prod_buf.as_mut_ptr() as *mut _,
                    (prod_buf.len() * 2) as u32,
                ) {
                    parse_utf16_z(&prod_buf)
                } else {
                    String::new()
                };

                let mut preparsed = PHIDP_PREPARSED_DATA::default();
                let mut caps = HIDP_CAPS::default();
                if HidD_GetPreparsedData(handle, &mut preparsed) {
                    let _ = HidP_GetCaps(preparsed, &mut caps);
                    let _ = HidD_FreePreparsedData(preparsed);
                }

                let _ = CloseHandle(handle);

                let interface_number = parse_interface_number(&path);
                out.push(HidDeviceInfo {
                    path,
                    vendor_id: attrs.VendorID,
                    product_id: attrs.ProductID,
                    version_number: attrs.VersionNumber,
                    usage_page: caps.UsagePage,
                    usage: caps.Usage,
                    input_report_len: caps.InputReportByteLength,
                    output_report_len: caps.OutputReportByteLength,
                    feature_report_len: caps.FeatureReportByteLength,
                    manufacturer,
                    product,
                    interface_number,
                });
            }

            let _ = SetupDiDestroyDeviceInfoList(dev_info);
        }
        out
    }

    pub struct HidHandle {
        handle: HANDLE,
    }

    impl Drop for HidHandle {
        fn drop(&mut self) {
            unsafe {
                if !self.handle.is_invalid() && self.handle != INVALID_HANDLE_VALUE {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }

    impl HidHandle {
        pub fn open(path: &str, overlapped: bool) -> Result<Self> {
            let wide = to_wide_null(path);
            let flags = if overlapped {
                FILE_FLAG_OVERLAPPED
            } else {
                FILE_ATTRIBUTE_NORMAL
            };
            unsafe {
                let handle = CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    flags,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("CreateFileW({path}) failed: {e}"))?;
                if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
                    bail!("CreateFileW({path}) returned invalid handle");
                }
                Ok(Self { handle })
            }
        }

        pub fn open_read_only(path: &str, overlapped: bool) -> Result<Self> {
            let wide = to_wide_null(path);
            let flags = if overlapped {
                FILE_FLAG_OVERLAPPED
            } else {
                FILE_ATTRIBUTE_NORMAL
            };
            unsafe {
                let handle = CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    flags,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("CreateFileW RO({path}) failed: {e}"))?;
                if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
                    bail!("CreateFileW RO({path}) returned invalid handle");
                }
                Ok(Self { handle })
            }
        }

        pub fn set_feature(&self, data: &[u8]) -> Result<()> {
            unsafe {
                if !HidD_SetFeature(self.handle, data.as_ptr() as *const _, data.len() as u32) {
                    let err = GetLastError();
                    bail!("HidD_SetFeature failed: err={}", err.0);
                }
            }
            Ok(())
        }

        pub fn get_feature(&self, report_id: u8, len: usize) -> Result<Vec<u8>> {
            let mut buf = vec![0u8; len.max(1)];
            buf[0] = report_id;
            unsafe {
                if !HidD_GetFeature(self.handle, buf.as_mut_ptr() as *mut _, buf.len() as u32) {
                    let err = GetLastError();
                    bail!("HidD_GetFeature failed: err={}", err.0);
                }
            }
            Ok(buf)
        }

        #[allow(dead_code)]
        pub fn set_output_report(&self, data: &[u8]) -> Result<()> {
            unsafe {
                if !HidD_SetOutputReport(
                    self.handle,
                    data.as_ptr() as *const _,
                    data.len() as u32,
                ) {
                    let err = GetLastError();
                    bail!("HidD_SetOutputReport failed: err={}", err.0);
                }
            }
            Ok(())
        }

        pub fn write_overlapped(&self, data: &[u8], timeout_ms: u32) -> Result<usize> {
            unsafe {
                let ev = CreateEventW(None, true, false, PCWSTR::null())
                    .map_err(|e| anyhow::anyhow!("CreateEventW failed: {e}"))?;
                let mut ov = OVERLAPPED {
                    hEvent: ev,
                    ..Default::default()
                };
                let mut written = 0u32;
                let write_res = WriteFile(
                    self.handle,
                    Some(data),
                    Some(&mut written),
                    Some(&mut ov),
                );
                let result = match write_res {
                    Ok(()) => Ok(written as usize),
                    Err(e) => {
                        let code = GetLastError();
                        if code == ERROR_IO_PENDING {
                            let wait = WaitForSingleObject(ev, timeout_ms);
                            if wait == WAIT_OBJECT_0 {
                                let mut done = 0u32;
                                GetOverlappedResult(self.handle, &ov, &mut done, false)?;
                                Ok(done as usize)
                            } else {
                                let _ = CancelIo(self.handle);
                                bail!("WriteFile timed out after {timeout_ms}ms");
                            }
                        } else {
                            Err(anyhow::anyhow!("WriteFile failed: {e} (err={})", code.0))
                        }
                    }
                };
                let _ = CloseHandle(ev);
                result
            }
        }

        pub fn read_overlapped(&self, len: usize, timeout_ms: u32) -> Result<Option<Vec<u8>>> {
            unsafe {
                let ev = CreateEventW(None, true, false, PCWSTR::null())
                    .map_err(|e| anyhow::anyhow!("CreateEventW failed: {e}"))?;
                let mut ov = OVERLAPPED {
                    hEvent: ev,
                    ..Default::default()
                };
                let mut buf = vec![0u8; len.max(1)];
                let mut got = 0u32;
                let read_res = ReadFile(
                    self.handle,
                    Some(&mut buf),
                    Some(&mut got),
                    Some(&mut ov),
                );
                let result = match read_res {
                    Ok(()) => {
                        buf.truncate(got as usize);
                        Ok(Some(buf))
                    }
                    Err(e) => {
                        let code = GetLastError();
                        if code == ERROR_IO_PENDING {
                            let wait = WaitForSingleObject(ev, timeout_ms);
                            if wait == WAIT_OBJECT_0 {
                                let mut done = 0u32;
                                match GetOverlappedResult(self.handle, &ov, &mut done, false) {
                                    Ok(()) => {
                                        buf.truncate(done as usize);
                                        Ok(Some(buf))
                                    }
                                    Err(err) => Err(anyhow::anyhow!("GetOverlappedResult: {err}")),
                                }
                            } else {
                                let _ = CancelIo(self.handle);
                                Ok(None)
                            }
                        } else {
                            Err(anyhow::anyhow!("ReadFile failed: {e} (err={})", code.0))
                        }
                    }
                };
                let _ = CloseHandle(ev);
                result
            }
        }

        /// Queue an overlapped ReadFile BEFORE sending the output packet so fast interrupt-IN
        /// responses from USB dongles are never missed, then drain until predicate matches.
        pub fn write_and_read_queued<F>(
            &self,
            out_data: &[u8],
            in_len: usize,
            total_timeout_ms: u32,
            max_reads: usize,
            mut predicate: F,
        ) -> Result<Option<Vec<u8>>>
        where
            F: FnMut(&[u8]) -> bool,
        {
            unsafe {
                let ev_r = CreateEventW(None, true, false, PCWSTR::null())
                    .map_err(|e| anyhow::anyhow!("CreateEventW read failed: {e}"))?;
                let mut ov_r = OVERLAPPED {
                    hEvent: ev_r,
                    ..Default::default()
                };
                let mut rbuf = vec![0u8; in_len.max(1)];
                let mut got = 0u32;
                let first_read = ReadFile(
                    self.handle,
                    Some(&mut rbuf),
                    Some(&mut got),
                    Some(&mut ov_r),
                );
                let first_err = if first_read.is_err() {
                    GetLastError()
                } else {
                    windows::Win32::Foundation::ERROR_SUCCESS
                };

                if first_read.is_err() && first_err != ERROR_IO_PENDING {
                    let _ = CloseHandle(ev_r);
                    bail!("Pre-queued ReadFile failed: err={}", first_err.0);
                }

                if let Err(e) = self.write_overlapped(out_data, 400) {
                    let _ = CancelIo(self.handle);
                    let _ = CloseHandle(ev_r);
                    return Err(e);
                }

                let deadline = Instant::now() + Duration::from_millis(total_timeout_ms as u64);

                // Check the pre-queued first read
                let first_pkt = if first_read.is_ok() {
                    rbuf.truncate(got as usize);
                    Some(rbuf)
                } else {
                    let rem = deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .max(1) as u32;
                    let wait = WaitForSingleObject(ev_r, rem);
                    if wait == WAIT_OBJECT_0 {
                        let mut done = 0u32;
                        if GetOverlappedResult(self.handle, &ov_r, &mut done, false).is_ok() {
                            rbuf.truncate(done as usize);
                            Some(rbuf)
                        } else {
                            None
                        }
                    } else {
                        let _ = CancelIo(self.handle);
                        None
                    }
                };
                let _ = CloseHandle(ev_r);

                if let Some(pkt) = first_pkt {
                    if predicate(&pkt) {
                        return Ok(Some(pkt));
                    }
                } else {
                    return Ok(None);
                }

                for _ in 1..max_reads {
                    let rem = deadline.saturating_duration_since(Instant::now());
                    if rem.is_zero() {
                        break;
                    }
                    let rem_ms = rem.as_millis().clamp(1, u32::MAX as u128) as u32;
                    match self.read_overlapped(in_len, rem_ms)? {
                        Some(pkt) => {
                            if predicate(&pkt) {
                                return Ok(Some(pkt));
                            }
                        }
                        None => break,
                    }
                }

                Ok(None)
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::{HidHandle, enumerate_hid_devices};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mi_from_path() {
        assert_eq!(
            parse_interface_number(
                r"\\?\hid#vid_5253&pid_1020&mi_02&col02#7&3795c50f&0&0001#{4d1e55b2}"
            ),
            Some(2)
        );
        assert_eq!(
            parse_interface_number(r"\\?\hid#vid_3537&pid_2106#7&2bb0f9df&0&0000#{4d1e55b2}"),
            None
        );
    }
}
