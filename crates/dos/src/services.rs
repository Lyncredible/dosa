use crate::{CONVENTIONAL_END, DosError, Machine, PSP, Pending, RunState, ZERO, files};
use dosa_mount::Entry;
use std::time::{SystemTime, UNIX_EPOCH};

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
            0x10 if self.video.bios(&mut self.cpu) => {}
            0x11 => self.cpu.regs.ax = 0x0020,
            0x12 => self.cpu.regs.ax = 640,
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
            0x20 => self.exit(0),
            0x21 => self.dos(ah, al),
            0x23 => self.exit(3),
            0x28 => {}
            0x29 => self.emit(&[al]),
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
        self.pending = Some(pending);
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
                let days = elapsed().as_secs() / 86_400;
                let (year, month, day) = civil_date(days);
                self.cpu.regs.cx = year;
                self.cpu.regs.dx = u16::from_le_bytes([day, month]);
                self.set_al(((days + 4) % 7) as u8);
            }
            0x2c => {
                let time = elapsed();
                let seconds = time.as_secs() % 86_400;
                self.cpu.regs.cx =
                    u16::from_le_bytes([(seconds / 60 % 60) as u8, (seconds / 3600) as u8]);
                self.cpu.regs.dx =
                    u16::from_le_bytes([(time.subsec_millis() / 10) as u8, (seconds % 60) as u8]);
            }
            0x2f => {
                self.cpu.regs.es = self.dta.0;
                self.cpu.regs.bx = self.dta.1;
            }
            0x30 => {
                self.cpu.regs.ax = 5;
                self.cpu.regs.bx = 0xff00;
                self.cpu.regs.cx = 0;
            }
            0x33 if al == 6 => {
                self.cpu.regs.bx = 5;
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
                if drive.is_some_and(|drive| self.mounts.contains_key(&drive)) {
                    self.cpu.regs.ax = 1;
                    self.cpu.regs.bx = 0;
                    self.cpu.regs.cx = 512;
                    self.cpu.regs.dx = 0xffff;
                } else {
                    self.cpu.regs.ax = 0xffff;
                }
            }
            0x38 => self.failure(1),
            0x39 | 0x3a | 0x3c | 0x41 | 0x56 | 0x5a | 0x5b => self.failure(5),
            0x3b => {
                let result = self
                    .path(ds, dx)
                    .and_then(|path| self.files.chdir(&self.mounts, &path));
                self.result(result);
            }
            0x3d => {
                if al & 3 != 0 {
                    self.failure(5);
                } else {
                    let result = self
                        .path(ds, dx)
                        .and_then(|path| self.files.open(&self.mounts, &path));
                    match result {
                        Ok(handle) => {
                            self.cpu.regs.ax = handle;
                            self.success();
                        }
                        Err(code) => self.failure(code),
                    }
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
                    self.cpu.load(ds, dx, bytes);
                    self.cpu.regs.ax = bytes.len() as u16;
                    self.success();
                }
                Err(code) => self.failure(code),
            },
            0x40 if bx == 1 || bx == 2 => {
                let bytes: Vec<_> = (0..cx)
                    .map(|i| self.cpu.read_u8(ds, dx.wrapping_add(i)))
                    .collect();
                self.emit(&bytes);
                self.cpu.regs.ax = cx;
                self.success();
            }
            0x40 => self.failure(if self.files.is_open(bx) { 5 } else { 6 }),
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
            0x57 if al == 0 && self.files.is_open(bx) => {
                self.cpu.regs.cx = 0;
                self.cpu.regs.dx = 0x0021;
                self.success();
            }
            _ => self.failure(1),
        }
    }

    fn result(&mut self, result: Result<(), u16>) {
        match result {
            Ok(()) => self.success(),
            Err(code) => self.failure(code),
        }
    }

    fn set_al(&mut self, value: u8) {
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

    fn find_result(&mut self, result: Result<Entry, u16>) {
        match result {
            Ok(entry) => {
                let (segment, offset) = self.dta;
                self.cpu.write_u8(
                    segment,
                    offset.wrapping_add(0x15),
                    files::attributes(&entry),
                );
                self.cpu.write_u16(segment, offset.wrapping_add(0x16), 0);
                self.cpu
                    .write_u16(segment, offset.wrapping_add(0x18), 0x0021);
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

fn elapsed() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn civil_date(mut days: u64) -> (u16, u8, u8) {
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mut year = 1970 + (days / 146_097 * 400) as u16;
    days %= 146_097;
    loop {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for (month, count) in months.into_iter().enumerate() {
        if days < count {
            return (year, month as u8 + 1, days as u8 + 1);
        }
        days -= count;
    }
    unreachable!()
}
