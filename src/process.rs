// SPDX-License-Identifier: Apache-2.0

use crate::device::Device;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

pub struct Process {
    pub gpu: usize,
    pub pid: u32,
    pub name: String,
    pub memory: Option<u64>,
}

fn fd_memory(text: &str) -> Option<(String, u64)> {
    let mut client = None;
    let mut memory = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("drm-client-id:") {
            client = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("drm-memory-vram:") {
            memory = value
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?
                .checked_mul(1024);
        }
    }
    Some((client?, memory?))
}

fn node_id(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    metadata
        .file_type()
        .is_char_device()
        .then_some(metadata.rdev())
}

pub fn discover(devices: &[Device]) -> Vec<Process> {
    let nodes: Vec<_> = devices
        .iter()
        .map(|device| {
            let card = Path::new("/dev/dri").join(&device.card);
            [node_id(&card), node_id(&device.node)]
        })
        .collect();
    let mut processes = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return processes;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let mut seen = vec![false; devices.len()];
        let mut clients = vec![HashSet::new(); devices.len()];
        let mut memory = vec![None::<u64>; devices.len()];
        for fd in fds.flatten() {
            let Some(id) = node_id(&fd.path()) else {
                continue;
            };
            for (gpu, candidate) in nodes.iter().enumerate() {
                if candidate.contains(&Some(id)) {
                    seen[gpu] = true;
                    if devices[gpu].driver == "amdgpu" {
                        if let Ok(info) =
                            fs::read_to_string(entry.path().join("fdinfo").join(fd.file_name()))
                        {
                            if let Some((client, bytes)) = fd_memory(&info) {
                                if clients[gpu].insert(client) {
                                    memory[gpu] =
                                        Some(memory[gpu].unwrap_or(0).saturating_add(bytes));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (gpu, present) in seen.iter().enumerate() {
            if *present {
                let name = fs::read_link(entry.path().join("exe"))
                    .ok()
                    .and_then(|path| path.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .or_else(|| {
                        fs::read_to_string(entry.path().join("comm"))
                            .ok()
                            .map(|s| s.trim().to_owned())
                    })
                    .unwrap_or_else(|| "N/A".to_owned());
                processes.push(Process {
                    gpu: devices[gpu].index,
                    pid,
                    name,
                    memory: memory[gpu],
                });
            }
        }
    }
    processes.sort_by_key(|p| (p.gpu, p.pid));
    processes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn finds_an_open_device_without_duplicate_rows() {
        let _first = File::open("/dev/null").unwrap();
        let _second = File::open("/dev/null").unwrap();
        let device = Device {
            index: 0,
            bus_id: String::new(),
            name: String::new(),
            driver: String::new(),
            sysfs: "/nonexistent".into(),
            node: "/dev/null".into(),
            card: "nonexistent".to_owned(),
        };
        let mine: Vec<_> = discover(&[device])
            .into_iter()
            .filter(|p| p.pid == std::process::id())
            .collect();
        assert_eq!(mine.len(), 1);
    }

    #[test]
    fn parses_amdgpu_fdinfo_memory() {
        assert_eq!(
            fd_memory("drm-client-id:\t93\ndrm-memory-vram:\t84792 KiB\n"),
            Some(("93".to_owned(), 84792 * 1024))
        );
        assert_eq!(fd_memory("drm-client-id:\t93\n"), None);
    }
}
