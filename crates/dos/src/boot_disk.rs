use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use dosa_cpu::Cpu;

const SECTOR_BYTES: usize = 512;
const MAX_TRANSFER: u16 = 127;
const BAD_COMMAND: u8 = 1;
const WRITE_PROTECTED: u8 = 3;
const SECTOR_NOT_FOUND: u8 = 4;
const MEDIA_CHANGED: u8 = 6;
const DMA_BOUNDARY: u8 = 9;
const DISK_FAILURE: u8 = 0x20;
const TIMEOUT: u8 = 0x80;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BootDrive {
    Floppy,
    HardDisk,
}

impl BootDrive {
    pub(super) fn number(self) -> u8 {
        match self {
            Self::Floppy => 0,
            Self::HardDisk => 0x80,
        }
    }
}

pub(super) struct BootDisk {
    file: File,
    drive: BootDrive,
    writable: bool,
    sectors: u64,
    cylinders: u16,
    heads: u16,
    track_sectors: u16,
    floppy_type: u8,
    status: u8,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct DiskSnapshot {
    pub media: DiskIdentity,
    pub status: u8,
}

#[derive(PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct DiskIdentity {
    drive: BootDrive,
    sectors: u64,
    checksum: u32,
}

impl BootDisk {
    pub(super) fn media_bytes(&self) -> u64 {
        self.sectors * SECTOR_BYTES as u64
    }

    pub(super) fn snapshot(&self) -> io::Result<DiskSnapshot> {
        let length = self.media_bytes();
        let mut file = self.file.try_clone()?;
        if file.metadata()?.len() != length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "boot image length changed",
            ));
        }
        file.rewind()?;
        let mut hasher = crc32fast::Hasher::new();
        if self.writable {
            // Keep old read-only identities stable and distinguish writable media.
            hasher.update(b"Dosa writable BIOS disk\0");
        }
        let mut buffer = [0; 64 * 1024];
        let mut remaining = length;
        while remaining != 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            let chunk = &mut buffer[..count];
            file.read_exact(chunk)?;
            hasher.update(chunk);
            remaining -= count as u64;
        }
        Ok(DiskSnapshot {
            media: DiskIdentity {
                drive: self.drive,
                sectors: self.sectors,
                checksum: hasher.finalize(),
            },
            status: self.status,
        })
    }

    pub(super) fn restore_status(&mut self, status: u8) {
        self.status = status;
    }

    pub fn open(
        file: File,
        drive: BootDrive,
        writable: bool,
    ) -> io::Result<(Self, [u8; SECTOR_BYTES])> {
        let (disk, sector) = Self::open_media(file, drive, writable)?;
        if sector[510..] != [0x55, 0xaa] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "boot sector is missing the 55 AA signature",
            ));
        }
        Ok((disk, sector))
    }

    pub(super) fn drive(&self) -> BootDrive {
        self.drive
    }

    pub(super) fn replace_floppy(&mut self, file: File) -> io::Result<()> {
        let (mut replacement, _) = Self::open_media(file, BootDrive::Floppy, false)?;
        replacement.status = self.status;
        *self = replacement;
        Ok(())
    }

    fn open_media(
        mut file: File,
        drive: BootDrive,
        writable: bool,
    ) -> io::Result<(Self, [u8; SECTOR_BYTES])> {
        let metadata = file.metadata()?;
        let length = metadata.len();
        if !metadata.is_file() || length == 0 || length % SECTOR_BYTES as u64 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a raw disk image with whole 512-byte sectors",
            ));
        }
        let sectors = length / SECTOR_BYTES as u64;
        let (cylinders, heads, track_sectors, floppy_type) = match drive {
            BootDrive::Floppy => floppy_geometry(sectors).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported raw floppy image size",
                )
            })?,
            BootDrive::HardDisk => {
                let heads = if sectors > 1024 * 16 * 63 { 255 } else { 16 };
                let cylinders = sectors.div_ceil(u64::from(heads) * 63).min(1024) as u16;
                (cylinders, heads, 63, 0)
            }
        };
        file.rewind()?;
        let mut sector = [0; SECTOR_BYTES];
        file.read_exact(&mut sector)?;
        Ok((
            Self {
                file,
                drive,
                writable,
                sectors,
                cylinders,
                heads,
                track_sectors,
                floppy_type,
                status: 0,
            },
            sector,
        ))
    }

    pub fn install_data(&self, cpu: &mut Cpu) {
        if self.drive == BootDrive::Floppy {
            crate::bios::install_floppy(cpu);
        }
        cpu.write_u8(0x40, 0x75, u8::from(self.drive == BootDrive::HardDisk));
    }

    pub fn interrupt(&mut self, cpu: &mut Cpu) {
        let [al, function] = cpu.regs.ax.to_le_bytes();
        let floppy = self.drive == BootDrive::Floppy;
        if function == 0x15 {
            let disk_type = if cpu.regs.dx as u8 != self.drive.number() {
                0
            } else if floppy {
                2
            } else {
                let sectors = self.sectors.min(u64::from(u32::MAX)) as u32;
                cpu.regs.cx = (sectors >> 16) as u16;
                cpu.regs.dx = sectors as u16;
                3
            };
            self.finish(cpu, 0);
            cpu.regs.ax |= disk_type << 8;
            return;
        }
        let status = if cpu.regs.dx as u8 != self.drive.number() {
            if function == 0x16 && (cpu.regs.dx as u8) < 0x80 {
                TIMEOUT
            } else {
                BAD_COMMAND
            }
        } else {
            match function {
                0x00 => 0,
                0x01 => self.status,
                0x02 | 0x03 if floppy && crate::bios::take_floppy_change(cpu) => {
                    cpu.regs.ax &= 0xff00;
                    MEDIA_CHANGED
                }
                0x02 | 0x03 => self.chs_transfer(cpu, al, function == 0x03),
                0x08 => {
                    let cylinder = self.cylinders - 1;
                    cpu.regs.cx =
                        (cylinder & 0xff) << 8 | (cylinder >> 2) & 0xc0 | self.track_sectors;
                    cpu.regs.dx = (self.heads - 1) << 8 | 1;
                    cpu.regs.bx = u16::from(self.floppy_type);
                    cpu.regs.es = 0;
                    cpu.regs.di = 0;
                    0
                }
                0x16 if floppy => {
                    if crate::bios::take_floppy_change(cpu) {
                        MEDIA_CHANGED
                    } else {
                        0
                    }
                }
                0x41 if self.drive == BootDrive::HardDisk && cpu.regs.bx == 0x55aa => {
                    cpu.regs.bx = 0xaa55;
                    cpu.regs.cx = 1;
                    self.finish(cpu, 0);
                    cpu.regs.ax = 0x2100;
                    return;
                }
                0x42 | 0x43 if self.drive == BootDrive::HardDisk => {
                    self.packet_transfer(cpu, function == 0x43, al)
                }
                0x48 if self.drive == BootDrive::HardDisk => self.extended_parameters(cpu),
                _ => BAD_COMMAND,
            }
        };
        self.finish(cpu, status);
    }

    fn finish(&mut self, cpu: &mut Cpu, status: u8) {
        self.status = status;
        let bda_status = if self.drive == BootDrive::Floppy {
            0x41
        } else {
            0x74
        };
        cpu.write_u8(0x40, bda_status, status);
        cpu.regs.ax = (cpu.regs.ax & 0xff) | u16::from(status) << 8;
        cpu.regs.flags = (cpu.regs.flags & !1) | u16::from(status != 0);
    }

    fn chs_transfer(&mut self, cpu: &mut Cpu, count: u8, write: bool) -> u8 {
        let [cl, ch] = cpu.regs.cx.to_le_bytes();
        let cylinder = u16::from(ch) | (u16::from(cl & 0xc0) << 2);
        let head = cpu.regs.dx >> 8;
        let sector = u16::from(cl & 0x3f);
        cpu.regs.ax &= 0xff00;
        if write && !self.writable {
            return WRITE_PROTECTED;
        }
        if cylinder >= self.cylinders
            || head >= self.heads
            || sector == 0
            || sector > self.track_sectors
        {
            return SECTOR_NOT_FOUND;
        }
        let first = (u64::from(cylinder) * u64::from(self.heads) + u64::from(head))
            * u64::from(self.track_sectors)
            + u64::from(sector - 1);
        let (status, transferred) = self.transfer(
            cpu,
            first,
            u16::from(count),
            cpu.regs.es,
            cpu.regs.bx,
            write,
        );
        cpu.regs.ax |= transferred;
        status
    }

    fn packet_transfer(&mut self, cpu: &mut Cpu, write: bool, flags: u8) -> u8 {
        let (ds, si) = (cpu.regs.ds, cpu.regs.si);
        if si > 0xfff0 || cpu.read_u16(ds, si) != 0x0010 {
            return BAD_COMMAND;
        }
        let count = cpu.read_u16(ds, si + 2);
        let offset = cpu.read_u16(ds, si + 4);
        let segment = cpu.read_u16(ds, si + 6);
        let lba = u64::from_le_bytes(std::array::from_fn(|index| {
            cpu.read_u8(ds, si + 8 + index as u16)
        }));
        // EDD 2.1 permits flags 0 and 1 for writes without verification.
        let (status, transferred) = if write && self.writable && flags > 1 {
            (BAD_COMMAND, 0)
        } else {
            self.transfer(cpu, lba, count, segment, offset, write)
        };
        cpu.write_u16(ds, si + 2, transferred);
        status
    }

    fn extended_parameters(&self, cpu: &mut Cpu) -> u8 {
        let (ds, si) = (cpu.regs.ds, cpu.regs.si);
        if usize::from(si) + 26 > 0x10000 || cpu.read_u16(ds, si) < 26 {
            return BAD_COMMAND;
        }
        let mut parameters = [0; 26];
        parameters[..2].copy_from_slice(&26u16.to_le_bytes());
        let geometry_sectors =
            u64::from(self.cylinders) * u64::from(self.heads) * u64::from(self.track_sectors);
        let flags = if self.sectors == geometry_sectors {
            2u16
        } else {
            0
        };
        parameters[2..4].copy_from_slice(&flags.to_le_bytes());
        for (field, value) in parameters[4..16].as_chunks_mut::<4>().0.iter_mut().zip([
            self.cylinders,
            self.heads,
            self.track_sectors,
        ]) {
            *field = u32::from(value).to_le_bytes();
        }
        parameters[16..24].copy_from_slice(&self.sectors.to_le_bytes());
        parameters[24..26].copy_from_slice(&(SECTOR_BYTES as u16).to_le_bytes());
        cpu.load(ds, si, &parameters);
        0
    }

    fn transfer(
        &mut self,
        cpu: &mut Cpu,
        first: u64,
        count: u16,
        segment: u16,
        offset: u16,
        write: bool,
    ) -> (u8, u16) {
        if write && !self.writable {
            return (WRITE_PROTECTED, 0);
        }
        if count == 0 || count > MAX_TRANSFER {
            return (BAD_COMMAND, 0);
        }
        if first
            .checked_add(u64::from(count))
            .is_none_or(|end| end > self.sectors)
        {
            return (SECTOR_NOT_FOUND, 0);
        }
        let size = usize::from(count) * SECTOR_BYTES;
        let physical_low = usize::from(segment.wrapping_shl(4).wrapping_add(offset));
        if physical_low + size > 0x10000 || usize::from(offset) + size > 0x10000 {
            return (DMA_BOUNDARY, 0);
        }
        if self
            .file
            .seek(SeekFrom::Start(first * SECTOR_BYTES as u64))
            .is_err()
        {
            return (DISK_FAILURE, 0);
        }
        if write {
            if !self
                .file
                .metadata()
                .is_ok_and(|metadata| metadata.len() == self.media_bytes())
            {
                return (DISK_FAILURE, 0);
            }
            let mut bytes = [0; SECTOR_BYTES];
            for sector in 0..count {
                let start = offset + sector * SECTOR_BYTES as u16;
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = cpu.read_u8(segment, start + index as u16);
                }
                if self.file.write_all(&bytes).is_err() {
                    return (DISK_FAILURE, sector);
                }
            }
            return (0, count);
        }
        let mut bytes = vec![0; size];
        if self.file.read_exact(&mut bytes).is_err() {
            return (DISK_FAILURE, 0);
        }
        cpu.load(segment, offset, &bytes);
        (0, count)
    }
}

fn floppy_geometry(sectors: u64) -> Option<(u16, u16, u16, u8)> {
    Some(match sectors {
        320 => (40, 1, 8, 1),
        360 => (40, 1, 9, 1),
        640 => (40, 2, 8, 1),
        720 => (40, 2, 9, 1),
        1440 => (80, 2, 9, 3),
        2400 => (80, 2, 15, 2),
        2880 => (80, 2, 18, 4),
        5760 => (80, 2, 36, 6),
        _ => return None,
    })
}
