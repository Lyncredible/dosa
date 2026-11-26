//! MSCDEX INT 2Fh services for mounted ISO 9660 data images.

use dosa_mount::{ISO_SECTOR_BYTES, Mount};

use crate::Machine;

const INVALID_FUNCTION: u16 = 1;
const BAD_FORMAT: u16 = 11;
const UNKNOWN_DRIVE: u16 = 15;
const DRIVE_NOT_READY: u16 = 21;
const VERSION: u16 = 0x0217; // MSCDEX 2.23
const MAX_TRANSFER: usize = 1 << 20;
const REQUEST_DONE: u16 = 0x0100;
const REQUEST_ERROR: u16 = 0x8000;
const UNKNOWN_COMMAND: u16 = 3;
const BAD_REQUEST_LENGTH: u16 = 5;
const SECTOR_NOT_FOUND: u16 = 8;
const READ_FAULT: u16 = 11;
const CD_LEAD_IN: u32 = 150;

impl Machine {
    pub(super) fn mscdex(&mut self, function: u8) {
        let mut drives = self
            .mounts
            .iter()
            .filter_map(|(&drive, mount)| mount.iso_sector_count().map(|_| drive as u8 - b'A'))
            .peekable();
        let first = drives.peek().copied();
        if function == 0 {
            self.cpu.regs.bx = drives.count() as u16;
            if let Some(first) = first {
                self.set_al(0xff);
                self.cpu.regs.cx = u16::from(first);
            }
            self.success();
            return;
        }
        if first.is_none() {
            self.set_al(0);
            return;
        }
        let regs = self.cpu.regs;
        let result = match function {
            5 => self.read_cdrom(16 + u32::from(regs.dx), 1, true),
            8 => self.read_cdrom(
                (u32::from(regs.si) << 16) | u32::from(regs.di),
                regs.dx,
                false,
            ),
            0x0b => {
                self.cpu.regs.ax = if drives.any(|drive| u16::from(drive) == regs.cx) {
                    0x5ad8
                } else {
                    0
                };
                self.cpu.regs.bx = 0xadad;
                Ok(())
            }
            0x0c => {
                self.cpu.regs.bx = VERSION;
                Ok(())
            }
            0x0d => {
                self.cpu.load(regs.es, regs.bx, &drives.collect::<Vec<_>>());
                Ok(())
            }
            0x10 => {
                let subunit = drives
                    .position(|drive| u16::from(drive) == regs.cx)
                    .map(|index| index as u8);
                self.cdrom_driver_request(subunit)
            }
            _ => Err(INVALID_FUNCTION),
        };
        match result {
            Ok(()) => self.success(),
            Err(code) => self.failure(code),
        }
    }

    fn read_cdrom(&mut self, first: u32, count: u16, descriptor: bool) -> Result<(), u16> {
        let regs = self.cpu.regs;
        let mount = self.cdrom(regs.cx)?;
        let bytes = read_sectors(mount, first, count).map_err(|code| match code {
            BAD_REQUEST_LENGTH => INVALID_FUNCTION,
            _ => DRIVE_NOT_READY,
        })?;
        let status = if descriptor {
            if &bytes[1..6] != b"CD001" {
                return Err(BAD_FORMAT);
            }
            match bytes[0] {
                1 => 1,
                0xff => 0xff,
                _ => 0,
            }
        } else {
            0
        };
        self.cpu.load(regs.es, regs.bx, &bytes);
        self.cpu.regs.ax = status;
        Ok(())
    }

    fn cdrom(&self, number: u16) -> Result<&Mount, u16> {
        if number < 26
            && let Some(mount) = self.drive(char::from(b'A' + number as u8))
            && mount.iso_sector_count().is_some()
        {
            Ok(mount)
        } else {
            Err(UNKNOWN_DRIVE)
        }
    }

    fn cdrom_driver_request(&mut self, subunit: Option<u8>) -> Result<(), u16> {
        let regs = self.cpu.regs;
        let subunit = subunit.ok_or(UNKNOWN_DRIVE)?;
        let length = self.cpu.read_u8(regs.es, regs.bx);
        let fits = |len| u16::from(length) >= len && regs.bx.checked_add(len - 1).is_some();
        // Without room for the status word, the caller cannot receive a driver error.
        if !fits(5) {
            return Err(BAD_FORMAT);
        }
        self.cpu.write_u8(regs.es, regs.bx + 1, subunit);
        let result = if !fits(13) {
            Err(BAD_REQUEST_LENGTH)
        } else {
            match self.cpu.read_u8(regs.es, regs.bx + 2) {
                3 if fits(20) => self.cdrom_control_input(),
                0x80 if fits(27) => self.cdrom_read_long(),
                3 | 0x80 => Err(BAD_REQUEST_LENGTH),
                0x0d | 0x0e => Ok(()),
                _ => Err(UNKNOWN_COMMAND),
            }
        };
        let status = REQUEST_DONE | result.err().map_or(0, |code| REQUEST_ERROR | code);
        self.cpu.write_u16(regs.es, regs.bx + 3, status);
        Ok(())
    }

    fn cdrom_control_input(&mut self) -> Result<(), u16> {
        let regs = self.cpu.regs;
        let offset = self.cpu.read_u16(regs.es, regs.bx + 14);
        let segment = self.cpu.read_u16(regs.es, regs.bx + 16);
        let count = self.cpu.read_u16(regs.es, regs.bx + 18);
        let fits = |len| count >= len && offset.checked_add(len - 1).is_some();
        if !fits(1) {
            return Err(BAD_REQUEST_LENGTH);
        }
        let function = self.cpu.read_u8(segment, offset);
        let mut result = [0; 4];
        let (start, size) = match function {
            6 if fits(5) => (1, 4), // Locked, loaded, read-only cooked media.
            7 if fits(4) => {
                if self.cpu.read_u8(segment, offset + 1) != 0 {
                    return Err(UNKNOWN_COMMAND);
                }
                result[..2].copy_from_slice(&(ISO_SECTOR_BYTES as u16).to_le_bytes());
                (2, 2)
            }
            8 if fits(5) => {
                let sectors = self.cdrom(regs.cx)?.iso_sector_count().unwrap();
                result = sectors.saturating_add(CD_LEAD_IN).to_le_bytes();
                (1, 4)
            }
            9 if fits(2) => {
                let drive = 1 << regs.cx;
                result[0] = if self.iso_media_changed & drive != 0 {
                    0xff
                } else {
                    1
                };
                self.iso_media_changed &= !drive;
                (1, 1)
            }
            6..=9 => return Err(BAD_REQUEST_LENGTH),
            _ => return Err(UNKNOWN_COMMAND),
        };
        self.cpu.load(segment, offset + start, &result[..size]);
        self.cpu
            .write_u16(regs.es, regs.bx + 18, start + size as u16);
        Ok(())
    }

    fn cdrom_read_long(&mut self) -> Result<(), u16> {
        let regs = self.cpu.regs;
        if self.cpu.read_u8(regs.es, regs.bx + 13) != 0
            || self.cpu.read_u8(regs.es, regs.bx + 24) != 0
        {
            return Err(UNKNOWN_COMMAND);
        }
        let offset = self.cpu.read_u16(regs.es, regs.bx + 14);
        let segment = self.cpu.read_u16(regs.es, regs.bx + 16);
        let count = self.cpu.read_u16(regs.es, regs.bx + 18);
        let first = u32::from(self.cpu.read_u16(regs.es, regs.bx + 20))
            | u32::from(self.cpu.read_u16(regs.es, regs.bx + 22)) << 16;
        let bytes = read_sectors(self.cdrom(regs.cx)?, first, count)?;
        self.cpu.load(segment, offset, &bytes);
        Ok(())
    }
}

fn read_sectors(mount: &Mount, first: u32, count: u16) -> Result<Vec<u8>, u16> {
    let len = usize::from(count) * ISO_SECTOR_BYTES;
    if len == 0 || len > MAX_TRANSFER {
        return Err(BAD_REQUEST_LENGTH);
    }
    let volume = mount.iso_sector_count().unwrap();
    if first
        .checked_add(u32::from(count))
        .is_none_or(|end| end > volume)
    {
        return Err(SECTOR_NOT_FOUND);
    }
    let mut bytes = vec![0; len];
    mount
        .read_iso_sectors(first, &mut bytes)
        .map_err(|_| READ_FAULT)?;
    Ok(bytes)
}
