use crate::DOS_VERSION;
use crate::{CONVENTIONAL_END, DosError, Machine, PSP, Pending, PendingCall, RunState, ZERO};
use dosa_mount::{Allocation, Entry};

impl Machine {
    pub(crate) fn interrupt(&mut self, vector: u8) -> Result<(), DosError> {
        let ax = self.cpu.regs.ax;
        let [al, ah] = ax.to_le_bytes();
        if self.mouse.owns_vector(&self.cpu, vector) {
            self.mouse_interrupt();
            return Ok(());
        }
        if self.has_interrupt_handler(vector) {
            self.cpu.enter_interrupt(vector);
            return Ok(());
        }
        match vector {
            0x10 if self.video.bios(&mut self.cpu) => {
                if ah == 0x0e && self.boot_disk.is_some() {
                    self.output.push(al);
                }
            }
            0x11 => self.cpu.regs.ax = crate::bios::equipment(&self.cpu),
            0x12 => self.cpu.regs.ax = 640,
            0x13 if let Some(disk) = &mut self.boot_disk => disk.interrupt(&mut self.cpu),
            0x16 => match ah {
                0 | 0x10 => self.resume(Pending::Bios),
                1 | 0x11 => {
                    self.cpu.regs.flags |= ZERO;
                    if let Some(&(ascii, scan)) = self.keys.front() {
                        self.cpu.regs.ax = u16::from_le_bytes([ascii, scan]);
                        self.cpu.regs.flags &= !ZERO;
                    }
                }
                2 => self.set_al(self.cpu.read_u8(0x40, 0x17)),
                0x12 => {
                    let flags = self.cpu.read_u8(0x40, 0x17);
                    let extended = self.cpu.read_u8(0x40, 0x18);
                    self.cpu.regs.ax = u16::from_le_bytes([flags, extended]);
                }
                3 => {}
                _ => return Err(DosError::Interrupt { vector, ax }),
            },
            0x1a if ah == 0 => {
                let (ticks, midnight) = crate::bios::take_timer(&mut self.cpu);
                self.cpu.regs.cx = (ticks >> 16) as u16;
                self.cpu.regs.dx = ticks as u16;
                self.set_al(midnight);
                self.success();
            }
            0x1c => {}
            0x18 if self.boot_disk.is_some() => self.exit(1),
            0x20..=0xff if self.boot_disk.is_some() => {
                return Err(DosError::Interrupt { vector, ax });
            }
            0x20 => self.exit(0),
            0x21 => self.dos(ah, al),
            0x23 => self.exit(3),
            0x28 => {}
            0x29 => self.emit(&[al]),
            0x2f if ah == 0x15 => self.mscdex(al),
            0x2f => self.cpu.regs.ax &= 0xff00,
            0x33 => {
                self.cpu.regs.ax = 0;
                self.cpu.regs.bx = 0;
            }
            0x67 => self.cpu.regs.ax = 0x8000,
            _ => return Err(DosError::Interrupt { vector, ax }),
        }
        Ok(())
    }

    pub(crate) fn resume(&mut self, pending: Pending) {
        if matches!(pending, Pending::Line) {
            self.read_line();
            return;
        }
        let Some((ascii, scan)) = self.keys.pop_front() else {
            self.wait(pending);
            return;
        };
        match pending {
            Pending::Bios => self.cpu.regs.ax = u16::from_le_bytes([ascii, scan]),
            Pending::Console { echo } => {
                let value = self.console_key(ascii, scan);
                self.set_al(value);
                if echo && value != 0 {
                    self.emit(&[value]);
                }
            }
            Pending::HandleRead => {
                let value = self.console_key(ascii, scan);
                self.cpu.write_u8(self.cpu.regs.ds, self.cpu.regs.dx, value);
                self.cpu.regs.ax = 1;
                self.success();
            }
            Pending::Line => unreachable!(),
        }
    }

    fn console_key(&mut self, ascii: u8, scan: u8) -> u8 {
        if ascii == 0 && scan != 0 {
            self.keys.push_front((scan, 0));
        }
        ascii
    }

    fn wait(&mut self, pending: Pending) {
        self.pending.push(PendingCall::new(pending, &self.cpu));
        self.state = RunState::WaitingForKey;
    }

    fn read_line(&mut self) {
        let (segment, offset) = (self.cpu.regs.ds, self.cpu.regs.dx);
        let limit = self.cpu.read_u8(segment, offset).saturating_sub(1) as usize;
        while let Some((ascii, _)) = self.keys.pop_front() {
            match ascii {
                b'\r' => {
                    self.cpu
                        .write_u8(segment, offset.wrapping_add(1), self.line.len() as u8);
                    self.cpu.load(segment, offset.wrapping_add(2), &self.line);
                    self.cpu.write_u8(
                        segment,
                        offset.wrapping_add(2 + self.line.len() as u16),
                        b'\r',
                    );
                    self.line.clear();
                    self.emit(b"\r\n");
                    return;
                }
                8 => {
                    if self.line.pop().is_some() {
                        self.emit(b"\x08 \x08");
                    }
                }
                32..=255 if self.line.len() < limit => {
                    self.line.push(ascii);
                    self.emit(&[ascii]);
                }
                _ => {}
            }
        }
        self.wait(Pending::Line);
    }

    fn dos(&mut self, ah: u8, al: u8) {
        let (bx, cx, dx, ds, es) = (
            self.cpu.regs.bx,
            self.cpu.regs.cx,
            self.cpu.regs.dx,
            self.cpu.regs.ds,
            self.cpu.regs.es,
        );
        let [dl, _] = dx.to_le_bytes();
        match ah {
            0x00 | 0x31 => self.exit(if ah == 0 { 0 } else { al }),
            0x01 => self.resume(Pending::Console { echo: true }),
            0x02 => {
                self.emit(&[dl]);
                self.set_al(dl);
            }
            0x06 if dl == 0xff => match self.keys.pop_front() {
                Some((ascii, scan)) => {
                    self.cpu.regs.flags &= !ZERO;
                    let value = self.console_key(ascii, scan);
                    self.set_al(value);
                }
                None => {
                    self.cpu.regs.flags |= ZERO;
                    self.set_al(0);
                }
            },
            0x06 => {
                self.emit(&[dl]);
                self.set_al(dl);
            }
            0x07 | 0x08 => self.resume(Pending::Console { echo: false }),
            0x09 => {
                let bytes = self.read_until(ds, dx, b'$', 1 << 16);
                self.emit(&bytes);
                self.set_al(b'$');
            }
            0x0a => {
                if self.cpu.read_u8(ds, dx) != 0 {
                    self.read_line();
                }
            }
            0x0b => self.set_al(if self.keys.is_empty() { 0 } else { 0xff }),
            0x0c => {
                self.keys.clear();
                if matches!(al, 1 | 6 | 7 | 8 | 0x0a) {
                    self.dos(al, al);
                } else {
                    self.set_al(0);
                }
            }
            0x0d => {}
            0x0e => {
                if let Some(letter) = drive_letter(dl) {
                    self.files.drive = letter;
                }
                self.set_al(26);
            }
            0x19 => self.set_al(self.files.drive as u8 - b'A'),
            0x1a => self.dta = (ds, dx),
            0x25 => {
                let vector = u16::from(al) * 4;
                self.cpu.write_u16(0, vector, dx);
                self.cpu.write_u16(0, vector + 2, ds);
            }
            0x2a => {
                let (year, month, day, weekday) = self.clock_snapshot().date();
                self.cpu.regs.cx = year;
                self.cpu.regs.dx = u16::from_le_bytes([day, month]);
                self.set_al(weekday);
            }
            0x2b => {
                let [day, month] = dx.to_le_bytes();
                let ok = self.set_date(cx, month, day);
                self.set_al(if ok { 0 } else { 0xff });
            }
            0x2c => {
                let (hour, minute, second, hundredths) = self.clock_snapshot().time();
                self.cpu.regs.cx = u16::from_le_bytes([minute, hour]);
                self.cpu.regs.dx = u16::from_le_bytes([hundredths, second]);
            }
            0x2d => {
                let [minute, hour] = cx.to_le_bytes();
                let [hundredths, second] = dx.to_le_bytes();
                let ok = self.set_time(hour, minute, second, hundredths);
                self.set_al(if ok { 0 } else { 0xff });
            }
            0x2e if al <= 1 => self.set_verify_writes(al == 1),
            0x2e => {}
            0x2f => {
                self.cpu.regs.es = self.dta.0;
                self.cpu.regs.bx = self.dta.1;
            }
            0x30 => {
                self.cpu.regs.ax = u16::from_le_bytes(DOS_VERSION);
                self.cpu.regs.bx = 0xff00;
                self.cpu.regs.cx = 0;
            }
            0x33 if al == 6 => {
                self.cpu.regs.bx = u16::from_le_bytes(DOS_VERSION);
                self.cpu.regs.dx = 0;
            }
            0x33 => self.cpu.regs.dx &= 0xff00,
            0x34 => {
                self.cpu.regs.es = 0x50;
                self.cpu.regs.bx = 0;
            }
            0x35 => {
                let vector = u16::from(al) * 4;
                self.cpu.regs.bx = self.cpu.read_u16(0, vector);
                self.cpu.regs.es = self.cpu.read_u16(0, vector + 2);
            }
            0x36 => {
                let drive = if dl == 0 {
                    Some(self.files.drive)
                } else {
                    drive_letter(dl - 1)
                };
                match drive.and_then(|drive| self.mounts.get(&drive)) {
                    Some(mount) => {
                        let allocation = mount.allocation().ok().flatten();
                        (
                            self.cpu.regs.ax,
                            self.cpu.regs.bx,
                            self.cpu.regs.cx,
                            self.cpu.regs.dx,
                        ) = disk_space_registers(allocation);
                    }
                    None => self.cpu.regs.ax = 0xffff,
                }
            }
            0x38 => self.failure(1),
            0x39..=0x3b => {
                let result = self.path(ds, dx).and_then(|path| match ah {
                    0x39 => self.files.mkdir(&self.mounts, &path),
                    0x3a => self.files.rmdir(&self.mounts, &path),
                    _ => self.files.chdir(&self.mounts, &path),
                });
                self.result(result);
            }
            0x3c | 0x3d => {
                let result = self.path(ds, dx).and_then(|path| {
                    if ah == 0x3c {
                        self.files
                            .create(&self.mounts, &path, self.adjusted_file_time())
                    } else {
                        self.files.open(&self.mounts, &path, al)
                    }
                });
                match result {
                    Ok(handle) => {
                        self.cpu.regs.ax = handle;
                        self.success();
                    }
                    Err(code) => self.failure(code),
                }
            }
            0x3e => {
                let result = self.files.close(bx);
                self.result(result);
            }
            0x3f if bx == 0 && cx != 0 => self.resume(Pending::HandleRead),
            0x3f if bx == 0 => {
                self.cpu.regs.ax = 0;
                self.success();
            }
            0x3f => match self.files.read(bx, usize::from(cx)) {
                Ok(bytes) => {
                    self.cpu.load(ds, dx, &bytes);
                    self.cpu.regs.ax = bytes.len() as u16;
                    self.success();
                }
                Err(code) => self.failure(code),
            },
            0x40 => {
                let console = matches!(bx, 1 | 2);
                if !console && let Err(code) = self.files.check_write_handle(bx) {
                    self.failure(code);
                    return;
                }
                let bytes: Vec<_> = (0..cx)
                    .map(|i| self.cpu.read_u8(ds, dx.wrapping_add(i)))
                    .collect();
                let result = if console {
                    self.emit(&bytes);
                    Ok(())
                } else {
                    let write = dosa_mount::FileWriteOptions {
                        modified: self.adjusted_file_time(),
                        verify: self.verify_writes(),
                    };
                    self.files.write(bx, &bytes, write)
                };
                if result.is_ok() {
                    self.cpu.regs.ax = cx;
                }
                self.result(result);
            }
            0x41 => {
                let result = self
                    .path(ds, dx)
                    .and_then(|path| self.files.delete(&self.mounts, &path));
                self.result(result);
            }
            0x42 => {
                let offset = ((u32::from(cx) << 16) | u32::from(dx)) as i32;
                match self.files.seek(bx, al, offset) {
                    Ok(position) => {
                        self.cpu.regs.ax = position as u16;
                        self.cpu.regs.dx = (position >> 16) as u16;
                        self.success();
                    }
                    Err(code) => self.failure(code),
                }
            }
            0x43 if al == 0 => {
                match self
                    .path(ds, dx)
                    .and_then(|path| self.files.attributes(&self.mounts, &path))
                {
                    Ok(attributes) => {
                        self.cpu.regs.cx = attributes;
                        self.success();
                    }
                    Err(code) => self.failure(code),
                }
            }
            0x43 => self.failure(5),
            0x44 if al == 0 && bx <= 4 => {
                self.cpu.regs.dx = match bx {
                    0 => 0x81,
                    1 | 2 => 0x82,
                    _ => 0x80,
                };
                self.success();
            }
            0x44 if al == 0 && self.files.is_open(bx) => {
                self.cpu.regs.dx = u16::from(self.files.drive as u8 - b'A');
                self.success();
            }
            0x44 if al == 8 => {
                self.cpu.regs.ax = 1;
                self.success();
            }
            0x44 => self.failure(1),
            0x47 => {
                let drive = if dl == 0 {
                    Some(self.files.drive)
                } else {
                    drive_letter(dl - 1)
                };
                if let Some(drive) = drive.filter(|drive| self.mounts.contains_key(drive)) {
                    let dir = self.files.directory(drive);
                    if dir.len() < 64 {
                        self.cpu.load(ds, self.cpu.regs.si, dir.as_bytes());
                        self.cpu
                            .write_u8(ds, self.cpu.regs.si.wrapping_add(dir.len() as u16), 0);
                        self.cpu.regs.ax = 0x0100;
                        self.success();
                    } else {
                        self.failure(3);
                    }
                } else {
                    self.failure(15);
                }
            }
            0x48 => self.allocate(bx),
            0x49 if es != PSP && self.blocks.remove(&es).is_some() => self.success(),
            0x49 => self.failure(9),
            0x4a => self.resize(es, bx),
            0x4c => self.exit(al),
            0x4d => self.cpu.regs.ax = 0,
            0x4e => {
                let entry = self
                    .path(ds, dx)
                    .and_then(|path| self.files.find(&self.mounts, &path, cx));
                self.find_result(entry);
            }
            0x4f => {
                let entry = self.files.next();
                self.find_result(entry);
            }
            0x51 | 0x62 => self.cpu.regs.bx = PSP,
            0x54 => self.set_al(u8::from(self.verify_writes())),
            0x56 => {
                let result = self.path(ds, dx).and_then(|from| {
                    self.path(self.cpu.regs.es, self.cpu.regs.di)
                        .and_then(|to| self.files.rename(&self.mounts, &from, &to))
                });
                self.result(result);
            }
            0x57 if al == 0 => match self.files.modified(bx) {
                Ok(timestamp) => {
                    (self.cpu.regs.dx, self.cpu.regs.cx) = timestamp.packed();
                    self.success();
                }
                Err(code) => self.failure(code),
            },
            0x5a | 0x5b => self.failure(5),
            // File writes are committed before returning from AH=40h.
            0x68 if self.files.is_open(bx) => self.success(),
            0x68 => self.failure(6),
            _ => self.failure(1),
        }
    }

    fn result(&mut self, result: Result<(), u16>) {
        match result {
            Ok(()) => self.success(),
            Err(code) => self.failure(code),
        }
    }

    pub(super) fn set_al(&mut self, value: u8) {
        self.cpu.regs.ax = (self.cpu.regs.ax & 0xff00) | u16::from(value);
    }

    fn read_until(&self, segment: u16, start: u16, end: u8, max: usize) -> Vec<u8> {
        (0..max)
            .map(|i| self.cpu.read_u8(segment, start.wrapping_add(i as u16)))
            .take_while(|&byte| byte != end)
            .collect()
    }

    fn path(&self, segment: u16, start: u16) -> Result<String, u16> {
        let bytes = self.read_until(segment, start, 0, 260);
        if bytes.is_empty() || bytes.len() == 260 || !bytes.is_ascii() {
            return Err(3);
        }
        String::from_utf8(bytes).map_err(|_| 3)
    }

    fn find_result(&mut self, result: Result<(Entry, u8), u16>) {
        match result {
            Ok((entry, attributes)) => {
                let (segment, offset) = self.dta;
                self.cpu
                    .write_u8(segment, offset.wrapping_add(0x15), attributes);
                let (date, time) = entry
                    .modified
                    .unwrap_or(dosa_mount::DosTimestamp::EPOCH)
                    .packed();
                self.cpu.write_u16(segment, offset.wrapping_add(0x16), time);
                self.cpu.write_u16(segment, offset.wrapping_add(0x18), date);
                let size = u32::try_from(entry.size).unwrap_or(u32::MAX);
                self.cpu
                    .load(segment, offset.wrapping_add(0x1a), &size.to_le_bytes());
                self.cpu.load(segment, offset.wrapping_add(0x1e), &[0; 13]);
                self.cpu
                    .load(segment, offset.wrapping_add(0x1e), entry.name.as_bytes());
                self.success();
            }
            Err(code) => self.failure(code),
        }
    }

    fn resize(&mut self, segment: u16, paragraphs: u16) {
        if !self.blocks.contains_key(&segment) || paragraphs == 0 {
            self.failure(9);
            return;
        }
        let next = self
            .blocks
            .range((
                std::ops::Bound::Excluded(segment),
                std::ops::Bound::Unbounded,
            ))
            .next()
            .map_or(CONVENTIONAL_END, |(&start, _)| start - 1);
        if paragraphs <= next - segment {
            self.blocks.insert(segment, paragraphs);
            if segment == PSP {
                self.cpu.write_u16(PSP, 2, PSP + paragraphs);
            }
            self.success();
        } else {
            self.cpu.regs.bx = next - segment;
            self.failure(8);
        }
    }

    fn allocate(&mut self, paragraphs: u16) {
        let gaps = self
            .blocks
            .iter()
            .zip(
                self.blocks
                    .keys()
                    .copied()
                    .skip(1)
                    .map(|start| start - 1)
                    .chain([CONVENTIONAL_END]),
            )
            .map(|((&start, &size), next)| {
                let usable = (start + size).saturating_add(1);
                (usable, next.saturating_sub(usable))
            });
        let mut maximum = 0;
        for (start, size) in gaps {
            maximum = maximum.max(size);
            if paragraphs > 0 && size >= paragraphs {
                self.blocks.insert(start, paragraphs);
                self.cpu.regs.ax = start;
                self.success();
                return;
            }
        }
        self.cpu.regs.bx = maximum;
        self.failure(8);
    }
}

fn drive_letter(number: u8) -> Option<char> {
    (number < 26).then(|| char::from(b'A' + number))
}

fn disk_space_registers(allocation: Option<Allocation>) -> (u16, u16, u16, u16) {
    let Some(allocation) = allocation else {
        return (1, 0, 512, u16::MAX);
    };
    let mut sectors = allocation.sectors_per_cluster;
    let bytes_per_sector = u64::from(allocation.bytes_per_sector);
    let max_clusters = u64::from(u16::MAX);
    let total = allocation.total_bytes();
    let free = allocation.free_bytes();
    // Widen allocation units as needed, up to the 32 KiB DOS cluster limit.
    // Any capacity beyond the 16-bit cluster count is outside this view.
    let mut cluster_bytes = u64::from(sectors) * bytes_per_sector;
    while total / cluster_bytes > max_clusters && cluster_bytes < 32 * 1024 {
        sectors *= 2;
        cluster_bytes *= 2;
    }
    let total_clusters = (total / cluster_bytes).min(max_clusters);
    let free_clusters = (free / cluster_bytes).min(total_clusters);
    (
        sectors,
        free_clusters as u16,
        allocation.bytes_per_sector,
        total_clusters as u16,
    )
}

#[cfg(test)]
mod disk_space_tests {
    use super::*;

    #[test]
    fn large_volumes_use_larger_units_without_overstating_free_space() {
        let allocation = Allocation {
            bytes_per_sector: 512,
            sectors_per_cluster: 1,
            free_clusters: 99_999,
            total_clusters: 100_000,
        };
        assert_eq!(
            disk_space_registers(Some(allocation)),
            (2, 49_999, 512, 50_000)
        );
        assert_eq!(
            disk_space_registers(Some(Allocation {
                total_clusters: u64::MAX,
                free_clusters: u64::MAX,
                ..allocation
            })),
            (64, u16::MAX, 512, u16::MAX)
        );
    }
}
