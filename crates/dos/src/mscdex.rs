//! MSCDEX INT 2Fh services for mounted ISO 9660 data images.

use dosa_mount::ISO_SECTOR_BYTES;

use crate::Machine;

const INVALID_FUNCTION: u16 = 1;
const BAD_FORMAT: u16 = 11;
const UNKNOWN_DRIVE: u16 = 15;
const DRIVE_NOT_READY: u16 = 21;
const VERSION: u16 = 0x0217; // MSCDEX 2.23
const MAX_TRANSFER: usize = 1 << 20;

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
            _ => Err(INVALID_FUNCTION),
        };
        match result {
            Ok(()) => self.success(),
            Err(code) => self.failure(code),
        }
    }

    fn read_cdrom(&mut self, first: u32, count: u16, descriptor: bool) -> Result<(), u16> {
        let regs = self.cpu.regs;
        let drive = (regs.cx < 26)
            .then(|| char::from(b'A' + regs.cx as u8))
            .ok_or(UNKNOWN_DRIVE)?;
        let mount = self.drive(drive).ok_or(UNKNOWN_DRIVE)?;
        let volume = mount.iso_sector_count().ok_or(UNKNOWN_DRIVE)?;
        let len = usize::from(count) * ISO_SECTOR_BYTES;
        if len == 0 || len > MAX_TRANSFER {
            return Err(INVALID_FUNCTION);
        }
        if first
            .checked_add(u32::from(count))
            .is_none_or(|end| end > volume)
        {
            return Err(DRIVE_NOT_READY);
        }
        let mut bytes = vec![0; len];
        mount
            .read_iso_sectors(first, &mut bytes)
            .map_err(|_| DRIVE_NOT_READY)?;
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
}
