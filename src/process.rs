// SPDX-License-Identifier: Apache-2.0

use crate::device::Device;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

pub struct Process {
    pub gpu: usize,
    pub pid: u32,
    pub name: String,
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
            [node_id(&card), device.node.as_deref().and_then(node_id)]
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
        for fd in fds.flatten() {
            let Some(id) = node_id(&fd.path()) else {
                continue;
            };
            for (gpu, candidate) in nodes.iter().enumerate() {
                if !seen[gpu] && candidate.contains(&Some(id)) {
                    seen[gpu] = true;
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
                    });
                }
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
            node: Some("/dev/null".into()),
            card: "nonexistent".to_owned(),
        };
        let mine: Vec<_> = discover(&[device])
            .into_iter()
            .filter(|p| p.pid == std::process::id())
            .collect();
        assert_eq!(mine.len(), 1);
    }
}
