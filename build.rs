use std::{env, fs, io, path::PathBuf};

const ICON: &str = "crates/desktop/assets/dosa.ico";
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed={ICON}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return Ok(());
    }

    let icon = fs::read(ICON)?;
    assert_eq!(&icon[..4], &[0, 0, 1, 0], "expected a Windows icon");
    let count = usize::from(u16::from_le_bytes(icon[4..6].try_into().unwrap()));
    let mut group = icon[..6].to_vec();
    let mut resources = Vec::new();
    resource(&mut resources, 0, 0, &[]);
    for (index, entry) in icon[6..6 + count * 16]
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
    {
        let id = u16::try_from(index + 1).expect("icon image count fits a resource identifier");
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let start = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        resource(&mut resources, RT_ICON, id, &icon[start..start + size]);
        group.extend_from_slice(&entry[..12]);
        group.extend(id.to_le_bytes());
    }
    resource(&mut resources, RT_GROUP_ICON, 1, &group);

    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("dosa.res");
    fs::write(&output, resources)?;
    println!("cargo:rustc-link-arg-bin=dosa={}", output.display());
    Ok(())
}

fn resource(output: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    // Windows .res header with integer type and name, followed by aligned data.
    output.extend((data.len() as u32).to_le_bytes());
    output.extend(32u32.to_le_bytes());
    for field in [0xffff, kind, 0xffff, id] {
        output.extend(field.to_le_bytes());
    }
    output.extend([0; 16]); // Data version, flags, language, version, characteristics
    output.extend_from_slice(data);
    output.resize(output.len().next_multiple_of(4), 0);
}
