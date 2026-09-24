// SPDX-License-Identifier: Apache-2.0

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const AMD_VENDOR: u16 = 0x1002;
const DRM_COMMAND_BASE: u64 = 0x40;
const RADEON_GEM_INFO: u64 = 0x1c;
const RADEON_INFO: u64 = 0x27;
const AMDGPU_INFO: u64 = 0x05;
const AMDGPU_INFO_READ_MMR_REG: u32 = 0x15;
const AMDGPU_INFO_DEV_INFO: u32 = 0x16;
const AMDGPU_INFO_SENSOR: u32 = 0x1d;
const INFO_VRAM_USAGE: u32 = 0x1e;
const INFO_GPU_SCLK: u32 = 0x22;
const INFO_GPU_MCLK: u32 = 0x23;
const INFO_READ_REG: u32 = 0x24;
const GRBM_STATUS: u32 = 0x8010;
const GUI_ACTIVE: u32 = 1 << 31;

#[repr(C)]
#[derive(Default)]
struct GemInfo {
    gart_size: u64,
    vram_size: u64,
    vram_visible: u64,
}

#[repr(C)]
struct RadeonInfo {
    request: u32,
    pad: u32,
    value: u64,
}

#[repr(C)]
struct AmdgpuInfo {
    return_pointer: u64,
    return_size: u32,
    query: u32,
    data: [u32; 4],
}

// Linux's generic _IOWR encoding, used by the x86_64 and aarch64 targets.
const fn drm_iowr(number: u64, size: usize) -> libc::c_ulong {
    ((3_u64 << 30) | ((size as u64) << 16) | ((b'd' as u64) << 8) | number) as libc::c_ulong
}

const fn drm_iow(number: u64, size: usize) -> libc::c_ulong {
    ((1_u64 << 30) | ((size as u64) << 16) | ((b'd' as u64) << 8) | number) as libc::c_ulong
}

const IOCTL_GEM_INFO: libc::c_ulong = drm_iowr(
    DRM_COMMAND_BASE + RADEON_GEM_INFO,
    std::mem::size_of::<GemInfo>(),
);
const IOCTL_INFO: libc::c_ulong = drm_iowr(
    DRM_COMMAND_BASE + RADEON_INFO,
    std::mem::size_of::<RadeonInfo>(),
);
// DRM_IOW, unlike the radeon queries above which use DRM_IOWR.
const IOCTL_AMDGPU_INFO: libc::c_ulong = drm_iow(
    DRM_COMMAND_BASE + AMDGPU_INFO,
    std::mem::size_of::<AmdgpuInfo>(),
);

#[derive(Clone, Debug)]
pub struct Device {
    pub index: usize,
    pub bus_id: String,
    pub name: String,
    pub driver: String,
    pub sysfs: PathBuf,
    pub node: PathBuf,
    pub card: String,
}

#[derive(Default)]
pub struct Metrics {
    pub temperature_c: Option<i64>,
    pub utilization: Option<u32>,
    pub memory_total: Option<u64>,
    pub memory_used: Option<u64>,
    pub gtt_total: Option<u64>,
    pub gtt_used: Option<u64>,
    pub visible_memory_total: Option<u64>,
    pub visible_memory_used: Option<u64>,
    pub fan_rpm: Option<u64>,
    pub graphics_mhz: Option<u32>,
    pub memory_mhz: Option<u32>,
    pub power_watts: Option<f64>,
    pub power_cap_watts: Option<f64>,
    pub display_active: Option<bool>,
    pub issue: Option<String>,
}

fn read_string(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_number(path: impl AsRef<Path>) -> Option<u64> {
    read_string(path)?.parse().ok()
}

fn read_hex(path: impl AsRef<Path>) -> Option<u16> {
    let value = read_string(path)?;
    u16::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

fn pci_name(device_id: u16, subsystem_vendor: u16, subsystem_id: u16) -> Option<String> {
    static IDS: OnceLock<Option<String>> = OnceLock::new();
    let ids = IDS
        .get_or_init(|| {
            ["/usr/share/misc/pci.ids", "/usr/share/hwdata/pci.ids"]
                .iter()
                .find_map(read_string)
        })
        .as_ref()?;
    let mut in_amd = false;
    let mut in_device = false;
    let mut device_name = None;
    for line in ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            in_amd = line.starts_with("1002  ");
            in_device = false;
        } else if in_amd && !line.starts_with("\t\t") {
            let id = format!("\t{device_id:04x}  ");
            in_device = line.starts_with(&id);
            if in_device {
                device_name = Some(line[id.len()..].trim().to_owned());
            }
        } else if in_device && line.starts_with("\t\t") {
            let id = format!("\t\t{subsystem_vendor:04x} {subsystem_id:04x}  ");
            if line.starts_with(&id) {
                return Some(line[id.len()..].trim().to_owned());
            }
        }
        if device_name.is_some() && !in_amd {
            break;
        }
    }
    device_name
}

pub fn discover() -> io::Result<Vec<Device>> {
    discover_at(Path::new("/sys/class/drm"), Path::new("/dev/dri"))
}

fn discover_at(drm: &Path, nodes: &Path) -> io::Result<Vec<Device>> {
    let mut devices = Vec::new();
    for entry in fs::read_dir(drm)? {
        let entry = entry?;
        let card = entry.file_name().to_string_lossy().into_owned();
        if !card.starts_with("card") || !card[4..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let sysfs = entry.path().join("device");
        if read_hex(sysfs.join("vendor")) != Some(AMD_VENDOR) {
            continue;
        }
        let Some(device_id) = read_hex(sysfs.join("device")) else {
            continue;
        };
        let bus_id = fs::canonicalize(&sysfs)
            .ok()
            .and_then(|path| path.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "unknown".to_owned());
        let uevent = read_string(sysfs.join("uevent")).unwrap_or_default();
        let driver = uevent
            .lines()
            .find_map(|line| line.strip_prefix("DRIVER="))
            .unwrap_or("unknown")
            .to_owned();
        if driver != "radeon" && driver != "amdgpu" {
            continue;
        }
        let name = pci_name(
            device_id,
            read_hex(sysfs.join("subsystem_vendor")).unwrap_or_default(),
            read_hex(sysfs.join("subsystem_device")).unwrap_or_default(),
        )
        .unwrap_or_else(|| format!("AMD GPU [1002:{device_id:04x}]"));
        let canonical = fs::canonicalize(&sysfs).ok();
        let render = fs::read_dir(drm).ok().and_then(|entries| {
            entries.flatten().find_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("renderD")
                    || canonical.is_none()
                    || fs::canonicalize(entry.path().join("device")).ok() != canonical
                {
                    return None;
                }
                Some(nodes.join(name))
            })
        });
        let node = render.unwrap_or_else(|| nodes.join(&card));
        devices.push(Device {
            index: 0,
            bus_id,
            name,
            driver,
            sysfs,
            node,
            card,
        });
    }
    devices.sort_by(|a, b| a.bus_id.cmp(&b.bus_id));
    for (index, device) in devices.iter_mut().enumerate() {
        device.index = index;
    }
    Ok(devices)
}

fn hwmon(device: &Device, field: &str) -> Option<u64> {
    for entry in fs::read_dir(device.sysfs.join("hwmon")).ok()?.flatten() {
        if let Some(value) = read_number(entry.path().join(field)) {
            return Some(value);
        }
    }
    None
}

fn display_active(device: &Device) -> Option<bool> {
    let parent = device.sysfs.parent()?.parent()?;
    let mut found = false;
    let mut active = false;
    for entry in fs::read_dir(parent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&format!("{}-", device.card)) {
            continue;
        }
        if let Some(enabled) = read_string(entry.path().join("enabled")) {
            found = true;
            active |= enabled == "enabled";
        }
    }
    found.then_some(active)
}

fn ioctl<T>(file: &File, request: libc::c_ulong, data: &mut T) -> io::Result<()> {
    // SAFETY: request numbers and repr(C) structures match Linux's public DRM UAPI.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, data as *mut T) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn radeon_info<T>(file: &File, request: u32, value: &mut T) -> io::Result<()> {
    let mut info = RadeonInfo {
        request,
        pad: 0,
        value: (value as *mut T) as u64,
    };
    ioctl(file, IOCTL_INFO, &mut info)
}

fn amdgpu_info<T>(file: &File, query: u32, data: [u32; 4], value: &mut T) -> io::Result<()> {
    let mut info = AmdgpuInfo {
        return_pointer: (value as *mut T) as u64,
        return_size: std::mem::size_of::<T>() as u32,
        query,
        data,
    };
    ioctl(file, IOCTL_AMDGPU_INFO, &mut info)
}

fn amdgpu_sensor(file: &File, sensor: u32) -> Option<u32> {
    let mut value = 0_u32;
    amdgpu_info(file, AMDGPU_INFO_SENSOR, [sensor, 0, 0, 0], &mut value)
        .ok()
        .map(|_| value)
}

fn sample_busy(file: &File, driver: &str) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut total = 0_u32;
    let mut busy = 0_u32;
    while Instant::now() < deadline {
        let mut status = GRBM_STATUS;
        let result = if driver == "radeon" {
            radeon_info(file, INFO_READ_REG, &mut status)
        } else {
            amdgpu_info(
                file,
                AMDGPU_INFO_READ_MMR_REG,
                [GRBM_STATUS / 4, 1, u32::MAX, 0],
                &mut status,
            )
        };
        if result.is_err() {
            return None;
        }
        total += 1;
        busy += u32::from(status & GUI_ACTIVE != 0);
        thread::sleep(Duration::from_millis(8));
    }
    (total >= 5).then_some((100 * busy + total / 2) / total)
}

pub fn collect(device: &Device) -> Metrics {
    let mut metrics = Metrics {
        temperature_c: hwmon(device, "temp1_input").map(|v| (v / 1000) as i64),
        graphics_mhz: hwmon(device, "freq1_input").map(|v| (v / 1_000_000) as u32),
        memory_mhz: hwmon(device, "freq2_input").map(|v| (v / 1_000_000) as u32),
        power_watts: hwmon(device, "power1_average").map(|v| v as f64 / 1_000_000.0),
        power_cap_watts: hwmon(device, "power1_cap").map(|v| v as f64 / 1_000_000.0),
        display_active: display_active(device),
        fan_rpm: hwmon(device, "fan1_input"),
        ..Metrics::default()
    };
    if device.driver == "amdgpu" {
        metrics.memory_total = read_number(device.sysfs.join("mem_info_vram_total"));
        metrics.memory_used = read_number(device.sysfs.join("mem_info_vram_used"));
        metrics.visible_memory_total = read_number(device.sysfs.join("mem_info_vis_vram_total"));
        metrics.visible_memory_used = read_number(device.sysfs.join("mem_info_vis_vram_used"));
        metrics.gtt_total = read_number(device.sysfs.join("mem_info_gtt_total"));
        metrics.gtt_used = read_number(device.sysfs.join("mem_info_gtt_used"));
        metrics.utilization = read_number(device.sysfs.join("gpu_busy_percent"))
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v <= 100);
    }
    let file = match OpenOptions::new().read(true).write(true).open(&device.node) {
        Ok(file) => file,
        Err(err) => {
            metrics.issue = Some(format!(
                "cannot open {}: {err}; check render/video device permissions",
                device.node.display()
            ));
            return metrics;
        }
    };
    if device.driver == "amdgpu" {
        metrics.graphics_mhz = amdgpu_sensor(&file, 1).or(metrics.graphics_mhz);
        metrics.memory_mhz = amdgpu_sensor(&file, 2).or(metrics.memory_mhz);
        metrics.temperature_c = amdgpu_sensor(&file, 3)
            .map(|v| (v / 1000) as i64)
            .or(metrics.temperature_c);
        if metrics.utilization.is_none() {
            let mut dev_info = [0_u32; 5];
            // GRBM_STATUS bit 31 describes graphics-pipe activity on GCN 1/2.
            if amdgpu_info(&file, AMDGPU_INFO_DEV_INFO, [0; 4], &mut dev_info).is_ok()
                && matches!(dev_info[4], 110 | 120)
            {
                metrics.utilization = sample_busy(&file, "amdgpu");
            }
        }
        return metrics;
    }
    let mut gem = GemInfo::default();
    if ioctl(&file, IOCTL_GEM_INFO, &mut gem).is_ok() {
        metrics.memory_total = Some(gem.vram_size);
    }
    let mut used = 0_u64;
    if radeon_info(&file, INFO_VRAM_USAGE, &mut used).is_ok() {
        metrics.memory_used = Some(used);
    }
    let mut clock = 0_u32;
    if radeon_info(&file, INFO_GPU_SCLK, &mut clock).is_ok() {
        metrics.graphics_mhz = Some(clock);
    }
    if radeon_info(&file, INFO_GPU_MCLK, &mut clock).is_ok() {
        metrics.memory_mhz = Some(clock);
    }
    metrics.utilization = sample_busy(&file, "radeon");
    if metrics.utilization.is_none() {
        metrics.issue = Some(
            "GPU utilization is unavailable through the radeon DRM query interface".to_owned(),
        );
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amdgpu_info_matches_linux_drm_uapi() {
        assert_eq!(std::mem::size_of::<AmdgpuInfo>(), 32);
        assert_eq!(IOCTL_AMDGPU_INFO, 0x4020_6445);
    }
}
