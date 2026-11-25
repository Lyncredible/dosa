use std::{error::Error, fmt::Write};

use dosa_mount::{Mount, VolumeIdentity};

use super::{Command, Shell};

pub(super) const HELP: &str =
    "VOL [drive:]\r\n    Display the volume label and serial number if they are available.\r\n";

impl Shell {
    pub(super) fn volume(&self, args: &str) -> Result<Command, Box<dyn Error>> {
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        if !args.is_empty() && !matches!(args.as_bytes(), [b'A'..=b'Z' | b'a'..=b'z', b':']) {
            return Err("usage: VOL [drive:]".into());
        }
        let (drive, _) = self.resolve(args)?;
        let mount = self.drive_mount(drive)?;
        Ok(Command::Print(volume_heading(drive, mount)?.into_bytes()))
    }
}

pub(super) fn volume_heading(drive: char, mount: &Mount) -> Result<String, Box<dyn Error>> {
    let VolumeIdentity { label, serial } = mount.volume_identity()?;
    let mut heading = match label {
        Some(label) => format!(" Volume in drive {drive} is {label}\r\n"),
        None => format!(" Volume in drive {drive} has no label\r\n"),
    };
    if let Some(serial) = serial {
        let _ = write!(
            heading,
            " Volume Serial Number is {:04X}-{:04X}\r\n",
            serial >> 16,
            serial & 0xffff
        );
    }
    Ok(heading)
}
