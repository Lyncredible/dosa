use cap_std::fs::Dir;

/// Identity exposed by mounted filesystems with an on-media volume label.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct VolumeIdentity {
    pub label: Option<String>,
    pub serial: Option<u32>,
}

impl VolumeIdentity {
    pub(crate) fn new(label: &str, serial: Option<u32>) -> Self {
        let label = label.trim_end_matches([' ', '\0']);
        let label = (!label.is_empty() && !label.eq_ignore_ascii_case("NO NAME")).then(|| {
            label
                .chars()
                .map(|ch| if ch.is_control() { '?' } else { ch })
                .collect()
        });
        Self { label, serial }
    }
}

/// Allocation geometry and counts reported by a mounted volume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Allocation {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u16,
    pub free_clusters: u64,
    pub total_clusters: u64,
}

impl Allocation {
    pub const fn free_bytes(self) -> u64 {
        self.free_clusters.saturating_mul(self.cluster_bytes())
    }

    pub const fn total_bytes(self) -> u64 {
        self.total_clusters.saturating_mul(self.cluster_bytes())
    }

    const fn cluster_bytes(self) -> u64 {
        self.bytes_per_sector as u64 * self.sectors_per_cluster as u64
    }

    fn host(free: u64, capacity: u64) -> Self {
        Self {
            bytes_per_sector: 512,
            sectors_per_cluster: 1,
            free_clusters: free.min(capacity) / 512,
            total_clusters: capacity / 512,
        }
    }
}

#[cfg(unix)]
pub(crate) fn host_space(root: &Dir) -> Option<Allocation> {
    let stat = rustix::fs::fstatvfs(root).ok()?;
    Some(Allocation::host(
        stat.f_bavail.saturating_mul(stat.f_frsize),
        stat.f_blocks.saturating_mul(stat.f_frsize),
    ))
}

#[cfg(windows)]
pub(crate) fn host_space(root: &Dir) -> Option<Allocation> {
    use std::{os::windows::io::AsRawHandle, ptr::null_mut};

    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetFinalPathNameByHandleW};

    let handle = root.as_raw_handle();
    let mut path = vec![0u16; 260];
    loop {
        // The live directory handle and the writable buffer remain valid for the call.
        let length =
            unsafe { GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, 0) }
                as usize;
        if length == 0 || length > 32_767 {
            return None;
        }
        if length < path.len() {
            path.truncate(length);
            break;
        }
        path.resize(length + 1, 0);
    }
    if !path.ends_with(&[u16::from(b'\\')]) {
        path.push(u16::from(b'\\'));
    }
    path.push(0);
    let mut free = 0;
    let mut capacity = 0;
    // The path is NUL-terminated and the requested outputs point to writable u64 values.
    let result =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut capacity, null_mut()) };
    (result != 0).then(|| Allocation::host(free, capacity))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn host_space(_: &Dir) -> Option<Allocation> {
    None
}
