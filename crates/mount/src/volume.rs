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

#[cfg(unix)]
pub(crate) fn available_space(root: &Dir) -> Option<u64> {
    let stat = rustix::fs::fstatvfs(root).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
pub(crate) fn available_space(root: &Dir) -> Option<u64> {
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
    // The path is NUL-terminated and the only requested output points to a u64.
    let result = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, null_mut(), null_mut()) };
    (result != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn available_space(_: &Dir) -> Option<u64> {
    None
}
