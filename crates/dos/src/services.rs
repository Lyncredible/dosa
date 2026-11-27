use crate::{CONVENTIONAL_END, DosError, Machine, PSP, Pending, PendingCall, RunState, ZERO, xms};
use crate::{
    CodePage, DOS_VERSION,
    files::{DosDevice, HandleTarget},
};
use dosa_mount::{Allocation, Entry};

impl Machine {
    pub(crate) fn interrupt(&mut self, vector: u8) -> Result<(), DosError> {
        let ax = self.cpu.regs.ax;
        let [al, ah] = ax.to_le_bytes();
        if self.cpu.protected_mode() {
            return self
                .cpu
                .enter_interrupt(vector)
                .ok_or(DosError::Interrupt { vector, ax });
        }
        if self.mouse.owns_vector(&self.cpu, vector) {
            self.mouse_interrupt();
            return Ok(());
        }
        if self.has_interrupt_handler(vector) {
            return self
                .cpu
                .enter_interrupt(vector)
                .ok_or(DosError::Interrupt { vector, ax });
        }
        match vector {
            0x10 if self.video.bios(&mut self.cpu) => {
                if ah == 0x0e && self.boot_disk.is_some() {
                    self.output.push(al);
                }
            }
            0x11 => self.cpu.regs.ax = crate::bios::equipment(&self.cpu),
            0x12 => self.cpu.regs.ax = 640,
            0x13 if self.cpu.regs.dx as u8 == 0x80
                && let Some(disk) = &mut self.installation_disk =>
            {
                disk.interrupt(&mut self.cpu);
            }
            0x13 if let Some(disk) = &mut self.boot_disk => disk.interrupt(&mut self.cpu),
            0x15 if ah == 0x24 => self.a20_interrupt(al),
            0x15 if ax == 0xe820 => crate::bios::memory_map(&mut self.cpu),
            0x15 if ah == 0x84 => self.joystick_interrupt(),
            0x15 if ah == 0x88 => {
                self.cpu.regs.ax = self.cpu.extended_memory_kib();
                self.success();
            }
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
            0x1a if ah <= 5 => self.clock_interrupt(ah),
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
            0x2f if ax == 0x4300 => self.set_al(if self.xms_available() { 0x80 } else { 0 }),
            0x2f if ax == 0x4310 && self.xms_available() => {
                self.cpu.regs.es = crate::xms::SEGMENT;
                self.cpu.regs.bx = crate::xms::ENTRY;
            }
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

    fn clock_interrupt(&mut self, function: u8) {
        use crate::bios::{bcd_pair, decode_bcd_pair, rtc_dst, set_rtc_dst, set_timer, take_timer};
        if function >= 2 && !self.cpu.has_a20() {
            self.failure(0x8600);
            return;
        }
        let cx = self.cpu.regs.cx;
        let dx = self.cpu.regs.dx;
        match function {
            0 => {
                let (ticks, midnight) = take_timer(&mut self.cpu);
                self.cpu.regs.cx = (ticks >> 16) as u16;
                self.cpu.regs.dx = ticks as u16;
                self.set_al(midnight);
            }
            1 => {
                let ticks = u32::from(cx) << 16 | u32::from(dx);
                set_timer(&mut self.cpu, ticks);
            }
            2 => {
                let (hours, minutes, seconds, _) = self.session_clock.now().time();
                self.cpu.regs.cx = bcd_pair(hours, minutes);
                self.cpu.regs.dx = bcd_pair(seconds, rtc_dst(&self.cpu));
            }
            3 if let Some(((hours, minutes), (seconds, dst))) =
                decode_bcd_pair(cx).zip(decode_bcd_pair(dx))
                && dst <= 1
                && self.set_time(hours, minutes, seconds, 0) =>
            {
                set_rtc_dst(&mut self.cpu, dst);
            }
            4 => {
                let (year, month, day, _) = self.session_clock.now().date();
                self.cpu.regs.cx = bcd_pair((year / 100) as u8, (year % 100) as u8);
                self.cpu.regs.dx = bcd_pair(month, day);
            }
            5 if let Some(((century, year), (month, day))) =
                decode_bcd_pair(cx).zip(decode_bcd_pair(dx))
                && self.set_date(u16::from(century) * 100 + u16::from(year), month, day) => {}
            _ => {
                self.failure(0x8600);
                return;
            }
        }
        self.success();
    }

    fn a20_interrupt(&mut self, function: u8) {
        if !self.cpu.has_a20() || function > 3 {
            self.failure(0x8600);
            return;
        }
        match function {
            0 => {
                self.fast_a20 = false;
                self.keyboard.set_a20(false);
                self.refresh_a20();
            }
            1 => {
                self.fast_a20 = true;
                self.refresh_a20();
            }
            3 => self.cpu.regs.bx = 3,
            _ => {}
        }
        self.cpu.regs.ax = u16::from(function == 2 && self.cpu.a20_enabled());
        self.success();
    }

    fn joystick_interrupt(&mut self) {
        match self.cpu.regs.dx {
            0 => {
                self.set_al(self.gameport.buttons());
                if !self.gameport.connected() {
                    self.cpu.regs.ax = 0x00f0;
                    self.cpu.regs.dx = crate::gameport::PORT;
                }
            }
            1 => {
                [
                    self.cpu.regs.ax,
                    self.cpu.regs.bx,
                    self.cpu.regs.cx,
                    self.cpu.regs.dx,
                ] = self.gameport.positions();
            }
            _ => {
                self.failure(0x8600);
                return;
            }
        }
        if self.gameport.connected() {
            self.success();
        } else {
            self.cpu.regs.flags |= crate::CARRY;
        }
    }

    pub(crate) fn resume(&mut self, pending: Pending) {
        match pending {
            Pending::Dos { ah, al } => return self.dos(ah, al),
            Pending::Line => return self.read_line(),
            Pending::Console { echo, check_break } if !self.console_stdin() => {
                if let Some(value) = self.read_redirected_byte() {
                    if value == 3 && check_break && self.extended_break() {
                        self.raise_break(pending);
                        return;
                    }
                    self.set_al(value);
                    if echo {
                        self.write_stdout(&[value]);
                    }
                } else {
                    self.set_al(0);
                }
                return;
            }
            _ => {}
        }
        if matches!(
            pending,
            Pending::Console {
                check_break: true,
                ..
            } | Pending::HandleRead
        ) && self.check_break(pending)
        {
            return;
        }
        let Some((ascii, scan)) = self.keys.pop_front() else {
            self.wait(pending);
            return;
        };
        match pending {
            Pending::Bios => self.cpu.regs.ax = u16::from_le_bytes([ascii, scan]),
            Pending::Console { echo, .. } => {
                let value = self.console_key(ascii, scan);
                self.set_al(value);
                if echo && value != 0 {
                    self.write_stdout(&[value]);
                }
            }
            Pending::HandleRead => {
                let value = self.console_key(ascii, scan);
                self.cpu.write_u8(self.cpu.regs.ds, self.cpu.regs.dx, value);
                self.cpu.regs.ax = 1;
                self.success();
            }
            Pending::Line | Pending::Dos { .. } => unreachable!(),
        }
    }

    fn check_break(&mut self, resume: Pending) -> bool {
        if !matches!(self.keys.front(), Some((3, _))) {
            return false;
        }
        self.keys.pop_front();
        self.raise_break(resume);
        true
    }

    fn raise_break(&mut self, resume: Pending) {
        self.emit(b"^C\r\n");
        if self.has_interrupt_handler(0x23) {
            // DOS resumes the interrupted service when the guest handler returns by IRET.
            let call = PendingCall::new(resume, &self.cpu);
            if self.cpu.enter_interrupt(0x23).is_some() {
                self.pending.push(call);
                return;
            }
        }
        self.exit(3);
    }

    fn console_stdin(&self) -> bool {
        matches!(self.files.input_target(0), Ok(HandleTarget::Console))
    }

    fn read_redirected_byte(&mut self) -> Option<u8> {
        if let Ok(HandleTarget::File(handle)) = self.files.input_target(0) {
            return self.files.read(handle, 1).ok()?.first().copied();
        }
        None
    }

    fn input_status(&self, handle: u16) -> bool {
        match self.files.input_target(handle) {
            Ok(HandleTarget::Console) => !self.keys.is_empty(),
            Ok(HandleTarget::File(file)) => self.files.available(file),
            _ => false,
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
        match self.files.input_target(0) {
            Ok(HandleTarget::Console) => {
                while let Some(&(ascii, _)) = self.keys.front() {
                    if self.check_break(Pending::Line) {
                        return;
                    }
                    self.keys.pop_front();
                    if self.line_byte(ascii, segment, offset, limit) {
                        return;
                    }
                }
                self.wait(Pending::Line);
                return;
            }
            Ok(HandleTarget::File(handle)) => {
                let extended_break = self.extended_break();
                let stop = Some(|byte| byte == b'\r' || extended_break && byte == 3);
                while let Ok(bytes) = self.files.read_through(handle, 256, stop) {
                    if bytes.is_empty() {
                        break;
                    }
                    for byte in bytes.into_owned() {
                        if byte == 3 && extended_break {
                            self.raise_break(Pending::Line);
                            return;
                        }
                        if self.line_byte(byte, segment, offset, limit) {
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
        self.finish_line(segment, offset);
    }

    fn line_byte(&mut self, ascii: u8, segment: u16, offset: u16, limit: usize) -> bool {
        match ascii {
            b'\r' => {
                self.finish_line(segment, offset);
                self.write_stdout(b"\r\n");
                return true;
            }
            8 => {
                if self.line.pop().is_some() {
                    self.write_stdout(b"\x08 \x08");
                }
            }
            b'\t' | 32..=255 if self.line.len() < limit => {
                self.line.push(ascii);
                self.write_stdout(&[ascii]);
            }
            _ => {}
        }
        false
    }

    fn finish_line(&mut self, segment: u16, offset: u16) {
        self.cpu
            .write_u8(segment, offset.wrapping_add(1), self.line.len() as u8);
        self.cpu.load(segment, offset.wrapping_add(2), &self.line);
        self.cpu.write_u8(
            segment,
            offset.wrapping_add(2 + self.line.len() as u16),
            b'\r',
        );
        self.line.clear();
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
        let console_check = matches!(ah, 2 | 5 | 9)
            || matches!(ah, 0x0b | 0x0c) && self.console_stdin()
            || (ah == 0x40
                && self.files.device(bx).is_some_and(|channel| {
                    channel.writable && channel.device == DosDevice::Console
                }));
        let extended_check = self.extended_break() && ah >= 0x0d && ah != 0x33;
        if (console_check || extended_check) && self.check_break(Pending::Dos { ah, al }) {
            return;
        }
        match ah {
            0x00 | 0x31 => self.exit(if ah == 0 { 0 } else { al }),
            0x01 => self.resume(Pending::Console {
                echo: true,
                check_break: true,
            }),
            0x02 => {
                self.write_stdout(&[dl]);
                self.set_al(dl);
            }
            0x06 if dl == 0xff => {
                let value = if self.console_stdin() {
                    self.keys
                        .pop_front()
                        .map(|(ascii, scan)| self.console_key(ascii, scan))
                } else {
                    self.read_redirected_byte()
                };
                if value.is_some() {
                    self.cpu.regs.flags &= !ZERO;
                } else {
                    self.cpu.regs.flags |= ZERO;
                }
                self.set_al(value.unwrap_or_default());
            }
            0x06 => {
                self.write_stdout(&[dl]);
                self.set_al(dl);
            }
            0x07 | 0x08 => self.resume(Pending::Console {
                echo: false,
                check_break: ah == 0x08,
            }),
            0x09 => {
                let bytes = self.read_until(ds, dx, b'$', 1 << 16);
                self.write_stdout(&bytes);
                self.set_al(b'$');
            }
            0x0a => {
                if self.cpu.read_u8(ds, dx) != 0 {
                    self.read_line();
                }
            }
            0x0b => self.set_al(if self.input_status(0) { 0xff } else { 0 }),
            0x0c => {
                if self.console_stdin() {
                    self.keys.clear();
                }
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
            0x33 if al == 0 => {
                self.cpu.regs.dx = (dx & 0xff00) | u16::from(self.extended_break());
            }
            0x33 if al == 1 => self.set_extended_break(dl & 1 != 0),
            0x33 if al == 6 => {
                self.cpu.regs.bx = u16::from_le_bytes(DOS_VERSION);
                self.cpu.regs.dx = 0;
            }
            0x33 => self.set_al(0xff),
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
                        self.sync_psp_handles();
                        self.success();
                    }
                    Err(code) => self.failure(code),
                }
            }
            0x3e => {
                let result = self.files.close(bx);
                if result.is_ok() {
                    self.sync_psp_handles();
                }
                self.result(result);
            }
            0x3f => self.read_handle(bx, cx, ds, dx),
            0x40 => self.write_handle(bx, cx, ds, dx),
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
            0x44 if al == 0 => match self.files.ioctl_info(bx) {
                Some(info) => {
                    self.cpu.regs.dx = info;
                    self.success();
                }
                None => self.failure(6),
            },
            0x44 if al == 6 && self.files.is_open(bx) => {
                self.set_al(if self.input_status(bx) { 0xff } else { 0 });
                self.success();
            }
            0x44 if al == 7
                && let Some(channel) = self.files.device(bx) =>
            {
                self.set_al(if channel.writable { 0xff } else { 0 });
                self.success();
            }
            0x44 if al == 8 => {
                self.cpu.regs.ax = 1;
                self.success();
            }
            0x44 => self.failure(1),
            0x45 => match self.files.duplicate(bx) {
                Ok(handle) => {
                    self.cpu.regs.ax = handle;
                    self.sync_psp_handles();
                    self.success();
                }
                Err(code) => self.failure(code),
            },
            0x46 => {
                let result = self.files.duplicate_to(bx, cx);
                if result.is_ok() {
                    self.sync_psp_handles();
                }
                self.result(result);
            }
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
            0x49 if es != self.psp() && self.blocks.remove(&es).is_some() => {
                if es >= xms::UPPER_START {
                    self.refresh_dos_umb_reservation();
                }
                self.success();
            }
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
            0x51 | 0x62 => self.cpu.regs.bx = self.psp(),
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
            0x58 => self.memory_control(al, bx),
            0x5a | 0x5b => self.failure(5),
            0x66 if al == 1 => {
                self.cpu.regs.bx = self.code_page().number();
                self.cpu.regs.dx = CodePage::default().number();
                self.success();
            }
            0x66 if al == 2 => {
                if let Some(page) = CodePage::from_number(bx) {
                    self.set_code_page(page);
                    self.success();
                } else {
                    self.failure(2);
                }
            }
            // File writes are committed before returning from AH=40h.
            0x68 if self.files.is_open(bx) => self.success(),
            0x68 => self.failure(6),
            _ => self.failure(1),
        }
    }

    fn sync_psp_handles(&mut self) {
        self.cpu.load(self.psp(), 0x18, &self.files.psp_handles());
    }

    fn read_handle(&mut self, handle: u16, count: u16, segment: u16, offset: u16) {
        match self.files.input_target(handle) {
            Ok(HandleTarget::Console) if count != 0 => self.resume(Pending::HandleRead),
            Ok(HandleTarget::Console | HandleTarget::Null) => {
                self.cpu.regs.ax = 0;
                self.success();
            }
            Ok(HandleTarget::File(file)) => match self.files.read(file, usize::from(count)) {
                Ok(bytes) => {
                    self.cpu.load(segment, offset, &bytes);
                    self.cpu.regs.ax = bytes.len() as u16;
                    self.success();
                }
                Err(code) => self.failure(code),
            },
            Err(code) => self.failure(code),
        }
    }

    fn write_handle(&mut self, handle: u16, count: u16, segment: u16, offset: u16) {
        let result = match self.files.output_target(handle) {
            Ok(HandleTarget::Null) => Ok(()),
            Ok(target) => {
                let bytes: Vec<_> = (0..count)
                    .map(|i| self.cpu.read_u8(segment, offset.wrapping_add(i)))
                    .collect();
                self.write_output(target, &bytes)
            }
            Err(code) => Err(code),
        };
        if result.is_ok() {
            self.cpu.regs.ax = count;
        }
        self.result(result);
    }

    fn write_stdout(&mut self, bytes: &[u8]) {
        if !bytes.is_empty()
            && let Ok(target) = self.files.output_target(1)
        {
            // Legacy character calls have no file-write error result.
            let _ = self.write_output(target, bytes);
        }
    }

    fn write_output(&mut self, target: HandleTarget, bytes: &[u8]) -> Result<(), u16> {
        match target {
            HandleTarget::Console => self.emit(bytes),
            HandleTarget::Null => {}
            HandleTarget::File(handle) => {
                let write = dosa_mount::FileWriteOptions {
                    modified: self.adjusted_file_time(),
                    verify: self.verify_writes(),
                };
                return self.files.write(handle, bytes, write);
            }
        }
        Ok(())
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

    fn memory_control(&mut self, subfunction: u8, value: u16) {
        match subfunction {
            0 => self.cpu.regs.ax = self.memory_strategy.code.into(),
            1 => {
                let Ok(code) = u8::try_from(value) else {
                    self.failure(1);
                    return;
                };
                if !crate::MemoryStrategy::valid_code(code) {
                    self.failure(1);
                    return;
                }
                self.memory_strategy.code = code;
                if self.upper_program.is_none() && self.boot_disk.is_none() {
                    self.session_memory_strategy.code = code;
                }
            }
            2 => self.set_al(u8::from(self.memory_strategy.linked)),
            3 if value <= 1 => {
                let linked = value == 1;
                if linked
                    && (!self.xms_available()
                        || self.upper_program.is_none() && !self.xms.upper_blocks().is_empty())
                {
                    self.failure(1);
                    return;
                }
                self.memory_strategy.linked = linked;
                if self.upper_program.is_none() && self.boot_disk.is_none() {
                    self.session_memory_strategy.linked = linked;
                }
                self.refresh_dos_umb_reservation();
            }
            _ => {
                self.failure(1);
                return;
            }
        }
        self.success();
    }

    pub(super) fn refresh_dos_umb_reservation(&mut self) {
        let end = Self::reserved_upper_end(self.upper_program, self.memory_strategy, &self.blocks);
        self.xms.reserve_upper_for_dos(end);
    }

    pub(super) fn reserved_upper_end(
        program: Option<crate::UpperProgram>,
        strategy: crate::MemoryStrategy,
        blocks: &std::collections::BTreeMap<u16, u16>,
    ) -> u16 {
        if let Some(program) = program {
            program.end
        } else if strategy.linked || blocks.range(xms::UPPER_START..).next().is_some() {
            xms::UPPER_END
        } else {
            xms::UPPER_START
        }
    }

    fn resize(&mut self, segment: u16, paragraphs: u16) {
        if !self.blocks.contains_key(&segment) || paragraphs == 0 {
            self.failure(9);
            return;
        }
        use std::ops::Bound::Excluded;
        let end = if segment >= xms::UPPER_START {
            Self::reserved_upper_end(self.upper_program, self.memory_strategy, &self.blocks)
        } else {
            CONVENTIONAL_END
        };
        if segment >= end {
            self.failure(9);
            return;
        }
        let next = self
            .blocks
            .range((Excluded(segment), Excluded(end)))
            .next()
            .map_or(end, |(&start, _)| start - 1);
        if paragraphs <= next - segment {
            self.blocks.insert(segment, paragraphs);
            if segment == self.psp() {
                self.cpu.write_u16(segment, 2, segment + paragraphs);
            }
            self.success();
        } else {
            self.cpu.regs.bx = next - segment;
            self.failure(8);
        }
    }

    fn allocate(&mut self, paragraphs: u16) {
        let mut maximum = 0;
        let strategy = self.memory_strategy.code;
        let upper = (strategy >= 0x40 && self.memory_strategy.linked).then(|| {
            let end =
                Self::reserved_upper_end(self.upper_program, self.memory_strategy, &self.blocks);
            xms::UPPER_START..end
        });
        let low = (strategy & 0x40 == 0
            && self
                .upper_program
                .is_none_or(|upper| upper.allow_conventional_allocations))
        .then_some(PSP..CONVENTIONAL_END);
        let fit = strategy & 0x3f;
        for arena in upper.into_iter().chain(low) {
            let mut selected = None;
            let mut usable = arena.start;
            let end = arena.end;
            let blocks = self
                .blocks
                .range(arena)
                .map(|(&start, &size)| (start, size));
            for (next, size) in blocks.chain([(end, 0)]) {
                let boundary = if size == 0 { end } else { next - 1 };
                let available = boundary.saturating_sub(usable);
                maximum = maximum.max(available);
                if paragraphs > 0 && available >= paragraphs {
                    match fit {
                        0 => {
                            selected = Some((usable, available));
                            break;
                        }
                        1 if selected.is_none_or(|(_, size)| available < size) => {
                            selected = Some((usable, available));
                        }
                        2 => selected = Some((boundary - paragraphs, available)),
                        _ => {}
                    }
                }
                usable = (next + size).saturating_add(1);
            }
            if let Some((segment, _)) = selected {
                self.blocks.insert(segment, paragraphs);
                self.cpu.regs.ax = segment;
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
