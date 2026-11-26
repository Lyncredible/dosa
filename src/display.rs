use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use dosa_desktop::{
    App, CpuMode, Frame, GamepadState, Input, Key, MenuAction, MenuState, PointerEvent,
    PointerMode, Scaling, Speed, StateSlot, TextMode, Volume,
};
use dosa_dos::{CpuProfile, JoystickState, MouseCursor, RunState, TextFrame, Video};
use dosa_mount::Mount;

use crate::shell::{Command, MAX_DESKTOP_SNAPSHOT_BYTES, PAUSE_MESSAGE, Shell};
use crate::wav::Recording;

const VGA: [[u8; 3]; 16] = [
    [17, 20, 28],
    [65, 103, 193],
    [109, 177, 116],
    [98, 174, 178],
    [192, 87, 96],
    [175, 106, 178],
    [188, 142, 85],
    [205, 212, 224],
    [106, 114, 134],
    [133, 164, 255],
    [152, 226, 157],
    [147, 228, 231],
    [255, 146, 153],
    [234, 159, 234],
    [255, 215, 151],
    [245, 248, 253],
];
const BATCH_LINES_PER_TICK: usize = 64;
const MAX_PASTE_CHARS: usize = 4096;

pub struct Desktop {
    shell: Shell,
    line: String,
    running: bool,
    paused: bool,
    inspecting: bool,
    volume: Volume,
    muted: bool,
    recording: Option<Recording>,
    scaling: Scaling,
    clipboard: Option<arboard::Clipboard>,
    paste: VecDeque<u8>,
    joystick_keys: [bool; 6],
    settings: Settings,
    status: String,
    state_slots: Option<PathBuf>,
    exit: bool,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    started: Instant,
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub cpu: CpuMode,
    pub speed: Speed,
    pub text_mode: TextMode,
    pub ansi_text: bool,
    pub virtual_joystick: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cpu: CpuMode::I386,
            speed: Speed::Normal,
            text_mode: TextMode::Rows25,
            ansi_text: false,
            virtual_joystick: false,
        }
    }
}

impl Settings {
    pub fn apply(self, shell: &mut Shell) -> Result<(), dosa_dos::DosError> {
        shell.machine.set_cpu_profile(cpu_profile(self.cpu))?;
        shell.machine.set_ansi_text(self.ansi_text);
        shell
            .machine
            .set_joystick(0, self.virtual_joystick.then(JoystickState::default));
        shell
            .machine
            .set_text_geometry(text_columns(self.text_mode), text_rows(self.text_mode));
        Ok(())
    }
}

impl Desktop {
    pub fn new(shell: Shell, initial: Option<&str>) -> Self {
        Self::initialize(shell, initial, Settings::default())
    }

    pub fn with_settings(
        mut shell: Shell,
        initial: Option<&str>,
        settings: Settings,
    ) -> Result<Self, dosa_dos::DosError> {
        settings.apply(&mut shell)?;
        Ok(Self::initialize(shell, initial, settings))
    }

    fn initialize(shell: Shell, initial: Option<&str>, settings: Settings) -> Self {
        let mut desktop = Self {
            shell,
            line: String::new(),
            running: false,
            paused: false,
            inspecting: false,
            volume: Volume::default(),
            muted: false,
            recording: None,
            scaling: Scaling::default(),
            clipboard: None,
            paste: VecDeque::new(),
            joystick_keys: [false; 6],
            settings,
            status: String::new(),
            state_slots: state_slot_directory(),
            exit: false,
            width: 640,
            height: 400,
            pixels: Vec::new(),
            started: Instant::now(),
        };
        desktop
            .shell
            .machine
            .write_console(b"dosa  |  Type HELP for commands.\r\n\r\n");
        desktop.write_prompt();
        if let Some(command) = initial {
            desktop.shell.machine.write_console(command.as_bytes());
            desktop.submit(command);
        }
        desktop.render();
        desktop
    }

    pub fn guest_graphics_active(&self) -> bool {
        self.running && matches!(self.shell.machine.video(), Video::Graphics { .. })
    }

    fn write_prompt(&mut self) {
        let prompt = self
            .shell
            .machine
            .code_page()
            .encode_text(&self.shell.prompt());
        self.shell.machine.write_console(&prompt);
    }

    fn refresh_prompt(&mut self, previous: &str) {
        if self.shell.prompt() != previous {
            self.shell.machine.write_console(b"\r\n");
            self.write_input_prompt();
        }
    }

    fn write_input_prompt(&mut self) {
        self.write_prompt();
        let typed = self.shell.machine.code_page().encode_text(&self.line);
        self.shell.machine.write_console(&typed);
    }

    fn finish_command(&mut self) {
        if let Err(error) = self.shell.finish_quick_launch() {
            self.status = format!("Quick launch cleanup failed: {error}");
        }
        self.write_input_prompt();
    }

    fn restore_state(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        load_guest_file(&mut self.shell, path)?;
        self.running = true;
        self.paused = self.shell.machine.paused_guest().is_some();
        self.inspecting = false;
        self.paste.clear();
        self.joystick_keys.fill(false);
        self.line.clear();
        self.settings.cpu = match self.shell.machine.cpu_profile() {
            CpuProfile::I8086 => CpuMode::I8086,
            CpuProfile::I286 => CpuMode::I286,
            CpuProfile::I386 => CpuMode::I386,
        };
        self.render();
        Ok(())
    }

    fn save_state_slot(&self, slot: StateSlot) -> Result<(), Box<dyn std::error::Error>> {
        let directory = self
            .state_slots
            .as_deref()
            .ok_or("no local state directory")?;
        std::fs::create_dir_all(directory)?;
        save_guest_file(&self.shell, &slot_file(directory, slot))
    }

    fn load_state_slot(&mut self, slot: StateSlot) -> Result<(), Box<dyn std::error::Error>> {
        let directory = self
            .state_slots
            .as_deref()
            .ok_or("no local state directory")?;
        let path = slot_file(directory, slot);
        self.restore_state(&path)
    }

    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, arboard::Error> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new()?);
        }
        Ok(self.clipboard.as_mut().unwrap())
    }

    fn copy_screen_text(&mut self) {
        let Video::Text(frame) = self.shell.machine.video() else {
            return;
        };
        let text = screen_text(frame);
        self.status = match self
            .clipboard()
            .and_then(|clipboard| clipboard.set_text(text))
        {
            Ok(()) => "Copied screen text".into(),
            Err(error) => format!("Copy failed: {error}"),
        };
    }

    fn paste_text(&mut self, text: &str) {
        let chars = paste_characters(text).take(MAX_PASTE_CHARS);
        if self.running {
            let page = self.shell.machine.code_page();
            self.paste = chars.map(|character| page.encode(character)).collect();
        } else if !self.shell.command_active() {
            for character in chars {
                let character = if matches!(character, '\r' | '\t') {
                    ' '
                } else {
                    character
                };
                self.line.push(character);
                let byte = self.shell.machine.code_page().encode(character);
                self.shell.machine.write_console(&[byte]);
            }
        }
    }

    fn submit(&mut self, line: &str) {
        self.shell.machine.write_console(b"\r\n");
        let result = self.shell.execute(line);
        self.apply_command(result);
        if !self.running && !self.exit && !self.shell.command_active() {
            self.write_prompt();
        }
    }

    fn apply_command(&mut self, result: Result<Command, Box<dyn std::error::Error>>) {
        match result {
            Ok(Command::Print(output)) => {
                self.shell.machine.write_console(&output);
                if !output.ends_with(b"\n") {
                    self.shell.machine.write_console(b"\r\n");
                }
            }
            Ok(Command::Clear) => self.shell.machine.clear_console(),
            Ok(Command::Pause) => self.shell.machine.write_console(PAUSE_MESSAGE),
            Ok(Command::WaitForInput(prompt)) => self.shell.machine.write_console(&prompt),
            Ok(Command::Prompt) => {}
            Ok(Command::Exit) => {
                self.exit = true;
            }
            Ok(Command::Launched) => {
                self.running = true;
                self.status = String::new();
            }
            Err(error) => self
                .shell
                .machine
                .write_console(format!("{error}\r\n").as_bytes()),
        }
    }

    fn advance_batch(&mut self) {
        for _ in 0..BATCH_LINES_PER_TICK {
            match self.shell.step_batch() {
                Ok(Some(step)) => {
                    if let Some(echo) = step.echo {
                        let echo = self.shell.machine.code_page().encode_text(&echo);
                        self.shell.machine.write_console(&echo);
                    }
                    self.apply_command(Ok(step.command));
                }
                Ok(None) => {
                    self.finish_command();
                    break;
                }
                Err(error) => {
                    self.apply_command(Err(error));
                    self.finish_command();
                    break;
                }
            }
            if self.running || self.exit || self.shell.batch_waiting() || self.shell.input_waiting()
            {
                break;
            }
        }
    }

    fn render(&mut self) {
        match self.shell.machine.video() {
            Video::Text(frame) => {
                self.width = u32::from(frame.columns) * 8;
                self.height = u32::from(frame.rows) * u32::from(frame.character_height);
                self.pixels
                    .resize((self.width * self.height * 4) as usize, 0);
                let blink = self.started.elapsed().as_millis() % 1000 < 500;
                render_text(frame, self.width as usize, &mut self.pixels, blink);
            }
            Video::Graphics {
                width,
                height,
                indices,
                palette,
            } => {
                self.width = width;
                self.height = height;
                self.pixels.resize((width * height * 4) as usize, 0);
                for (&index, pixel) in indices.iter().zip(self.pixels.as_chunks_mut::<4>().0) {
                    let [r, g, b] = palette[usize::from(index)];
                    pixel.copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
        if self.running
            && !self.paused
            && let Some(cursor) = self.shell.machine.mouse_cursor()
        {
            draw_mouse_cursor(&mut self.pixels, self.width, self.height, &cursor);
        }
    }

    fn set_text_mode(&mut self, mode: TextMode) {
        if mode == self.settings.text_mode {
            return;
        }
        if !self
            .shell
            .machine
            .set_text_geometry(text_columns(mode), text_rows(mode))
        {
            return;
        }
        self.settings.text_mode = mode;
        self.shell.machine.clear_console();
        self.write_prompt();
        for character in self.line.chars() {
            let byte = self.shell.machine.code_page().encode(character);
            self.shell.machine.write_console(&[byte]);
        }
    }

    fn finish_recording(&mut self, limit_reached: bool) {
        if let Some(recording) = self.recording.take() {
            self.status = match recording.finish() {
                Ok(()) if limit_reached => "WAV saved at size limit".into(),
                Ok(()) => "WAV saved".into(),
                Err(error) => format!("WAV save failed: {error}"),
            };
        }
    }

    fn joystick_input(&mut self, scan: u8, pressed: bool) -> bool {
        if !self.settings.virtual_joystick {
            return false;
        }
        let key = match scan {
            0x4b => 0, // Left
            0x4d => 1, // Right
            0x48 => 2, // Up
            0x50 => 3, // Down
            0x2c => 4, // Z
            0x2d => 5, // X
            _ => return false,
        };
        if !self.paused || !pressed {
            self.joystick_keys[key] = pressed;
            let [left, right, up, down, first, second] = self.joystick_keys;
            let axis = |negative, positive| match (negative, positive) {
                (true, false) => 0,
                (false, true) => 255,
                _ => 128,
            };
            self.shell.machine.set_joystick(
                0,
                Some(JoystickState {
                    x: axis(left, right),
                    y: axis(up, down),
                    buttons: [first, second],
                }),
            );
        }
        true
    }

    fn center_keyboard_joystick(&mut self) {
        if self.settings.virtual_joystick {
            self.joystick_keys.fill(false);
            self.shell
                .machine
                .set_joystick(0, Some(JoystickState::default()));
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.shell.machine.set_paused(paused);
        if paused || !self.running {
            self.center_keyboard_joystick();
        }
        if paused {
            self.shell.machine.mouse_buttons(0);
        } else {
            self.inspecting = false;
        }
    }

    fn reset_guest(&mut self) {
        self.running = false;
        self.paused = false;
        self.inspecting = false;
        self.paste.clear();
        self.joystick_keys.fill(false);
        self.line.clear();
        self.status = match self
            .shell
            .reset()
            .and_then(|()| self.settings.apply(&mut self.shell))
        {
            Ok(()) => "Guest reset".into(),
            Err(error) => format!("Reset failed: {error}"),
        };
        self.write_prompt();
        self.render();
    }

    fn write_blocked_zip(&self) -> Option<char> {
        self.shell.machine.denied_write_drive().filter(|&drive| {
            self.shell
                .machine
                .drive(drive)
                .is_some_and(Mount::is_read_only_zip)
        })
    }

    fn use_writable_zip(&mut self, drive: char) -> Result<(), Box<dyn std::error::Error>> {
        let copy = self
            .shell
            .machine
            .drive(drive)
            .ok_or(dosa_dos::DosError::UnmountedDrive(drive))?
            .writable_zip_copy()?;
        if self.running || self.shell.command_active() {
            // The menu explicitly stops the current command. Prepare the copy first
            // so an extraction failure leaves the guest and original mount intact.
            self.shell.machine.stop();
            let _ = self
                .shell
                .guest_failed("guest stopped for a writable ZIP copy".into());
            self.update_guest(Ok(RunState::Exited(3)));
        }
        self.shell.machine.mount(drive, copy)?;
        Ok(())
    }

    fn update_guest(&mut self, result: Result<RunState, dosa_dos::DosError>) {
        match result {
            Ok(state @ (RunState::Idle | RunState::Exited(_))) => {
                if let RunState::Exited(code) = state {
                    self.shell.guest_finished(code);
                }
                self.running = false;
                self.set_paused(false);
                self.paste.clear();
                self.shell.machine.write_console(b"\r\n");
                if !self.shell.command_active() {
                    self.finish_command();
                }
            }
            Ok(RunState::Running | RunState::WaitingForKey | RunState::WaitingForInterrupt) => {}
            Err(error) => {
                self.running = false;
                self.set_paused(false);
                self.paste.clear();
                self.shell.machine.stop();
                let error = self.shell.guest_failed(error.into());
                self.shell
                    .machine
                    .write_console(format!("\r\n{error}\r\n").as_bytes());
                self.finish_command();
            }
        }
        // The guest's output has already been written to video memory.
        self.shell.machine.take_output();
    }

    fn inspection(&self) -> Vec<String> {
        if !self.inspecting {
            return Vec::new();
        }
        let Some(guest) = self.shell.machine.paused_guest() else {
            return Vec::new();
        };
        let r = guest.registers();
        let ip = guest.instruction_pointer();
        let sp = guest.stack_pointer();
        let memory = |label, segment, offset: u32, peek: &dyn Fn(u8) -> Option<u8>| {
            use std::fmt::Write;
            let mut line = format!("{label} {segment:04X}:{offset:04X}");
            for index in 0..8 {
                match peek(index) {
                    Some(byte) => write!(line, " {byte:02X}").unwrap(),
                    None => line.push_str(" ??"),
                }
            }
            line
        };
        let state = match guest.state {
            RunState::WaitingForKey => "Waiting for keyboard input",
            RunState::WaitingForInterrupt => "Waiting for an interrupt",
            _ => "Paused at instruction boundary",
        };
        vec![
            state.into(),
            format!(
                "AX {:04X}  BX {:04X}  CX {:04X}  DX {:04X}",
                r.ax, r.bx, r.cx, r.dx
            ),
            format!(
                "SI {:04X}  DI {:04X}  BP {:04X}  SP {:04X}",
                r.si, r.di, r.bp, sp
            ),
            format!(
                "CS {:04X}  DS {:04X}  ES {:04X}  SS {:04X}",
                r.cs, r.ds, r.es, r.ss
            ),
            format!("IP {:04X}  FLAGS {:04X}", ip, r.flags),
            memory("Code ", r.cs, ip, &|index| guest.peek_code(index)),
            memory("Data ", r.ds, guest.source_pointer(), &|index| {
                guest.peek_data(index)
            }),
            memory("Stack", r.ss, sp, &|index| guest.peek_stack(index)),
        ]
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if let Some(recording) = self.recording.take()
            && let Err(error) = recording.finish()
        {
            eprintln!("WAV save failed: {error}");
        }
    }
}

impl App for Desktop {
    fn audio(&mut self) -> Vec<f32> {
        let samples = self.shell.machine.take_audio();
        if let Some(recording) = &mut self.recording
            && !samples.is_empty()
        {
            match recording.append(&samples) {
                Ok(false) => {}
                Ok(true) => self.finish_recording(true),
                Err(error) => {
                    self.recording = None;
                    self.status = format!("WAV recording failed: {error}");
                }
            }
        }
        if self.running && !self.paused {
            samples
        } else {
            Vec::new()
        }
    }

    fn audio_error(&mut self, error: &str) {
        self.status = format!("Audio unavailable: {error}");
    }

    fn tick(&mut self) {
        if self.running && !self.paused {
            if let Some(&ascii) = self.paste.front() {
                let scan = match ascii {
                    b'\r' => 0x1c,
                    b'\t' => 0x0f,
                    _ => 0,
                };
                if self.shell.machine.paste_key(ascii, scan) {
                    self.paste.pop_front();
                }
            }
            let result = self
                .shell
                .machine
                .run(instructions_per_tick(self.settings.speed));
            self.update_guest(result);
        }
        if !self.running
            && !self.paused
            && !self.exit
            && self.shell.command_active()
            && !self.shell.batch_waiting()
            && !self.shell.input_waiting()
        {
            if self.recording.is_some() {
                // Capture the last samples before the next guest resets the audio devices.
                self.audio();
            }
            self.advance_batch();
        }
        self.render();
    }

    fn frame(&self) -> Frame<'_> {
        Frame {
            width: self.width,
            height: self.height,
            rgba: &self.pixels,
        }
    }

    fn display_aspect(&self) -> Option<f32> {
        matches!(self.shell.machine.video(), Video::Graphics { .. }).then_some(4.0 / 3.0)
    }

    fn should_exit(&self) -> bool {
        self.exit
    }

    fn menu(&self) -> MenuState {
        let write_blocked_zip = self.write_blocked_zip();
        MenuState {
            enabled: true,
            active_drive: self.shell.current_drive(),
            write_blocked_zip,
            drives: ('A'..='Z')
                .filter_map(|letter| {
                    self.shell
                        .machine
                        .drive(letter)
                        .map(|mount| (letter, mount.description()))
                })
                .collect(),
            cpu: self.settings.cpu,
            speed: self.settings.speed,
            volume: self.volume,
            muted: self.muted,
            recording: self.recording.is_some(),
            text_mode: self.settings.text_mode,
            scaling: self.scaling,
            running: self.running,
            batch_active: self.shell.command_active(),
            save_state_blocked: !self.shell.supports_save_state(),
            state_slots_available: self.state_slots.is_some(),
            text_visible: matches!(self.shell.machine.video(), Video::Text(_)),
            paste_pending: !self.paste.is_empty(),
            paused: self.paused,
            inspection: self.inspection(),
            status: match write_blocked_zip {
                Some(drive) if self.status.is_empty() => {
                    format!("{drive}: write denied. See Drives.")
                }
                _ => self.status.clone(),
            },
        }
    }

    fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::CopyText => self.copy_screen_text(),
            MenuAction::PasteText if self.running || !self.shell.command_active() => {
                match self.clipboard().and_then(arboard::Clipboard::get_text) {
                    Ok(text) => self.paste_text(&text),
                    Err(error) => self.status = format!("Paste failed: {error}"),
                }
            }
            MenuAction::StopPaste => self.paste.clear(),
            MenuAction::UseWritableZip(drive) if self.write_blocked_zip() == Some(drive) => {
                self.status = match self.use_writable_zip(drive) {
                    Ok(()) => format!("{drive}: writable ZIP. Run the program again."),
                    Err(error) => format!("ZIP copy failed: {error}"),
                };
            }
            MenuAction::QuickLaunch(path) if !self.running && !self.shell.command_active() => {
                match self.shell.quick_launch(&path) {
                    Ok(command) => {
                        self.shell.machine.write_console(b"\r\n");
                        self.apply_command(Ok(command));
                    }
                    Err(error) => self.status = format!("Quick launch failed: {error}"),
                }
            }
            MenuAction::SaveScreenshot(path) => {
                self.render();
                self.status = match save_screenshot(&path, self.frame()) {
                    Ok(()) => "Screenshot saved".into(),
                    Err(error) => format!("Screenshot failed: {error}"),
                };
            }
            MenuAction::SaveGuestState(path) if self.running => {
                self.status = match save_guest_file(&self.shell, &path) {
                    Ok(()) => "State saved".into(),
                    Err(error) => format!("Save state failed: {error}"),
                };
            }
            MenuAction::LoadGuestState(path) => {
                self.status = match self.restore_state(&path) {
                    Ok(()) => "State loaded".into(),
                    Err(error) => format!("Load state failed: {error}"),
                };
            }
            MenuAction::SaveStateSlot(slot) if self.running => {
                self.status = match self.save_state_slot(slot) {
                    Ok(()) => format!("Slot {} saved", slot.number()),
                    Err(error) => format!("Save slot {} failed: {error}", slot.number()),
                };
            }
            MenuAction::LoadStateSlot(slot) => {
                self.status = match self.load_state_slot(slot) {
                    Ok(()) => format!("Slot {} loaded", slot.number()),
                    Err(error) => format!("Load slot {} failed: {error}", slot.number()),
                };
            }
            MenuAction::Mount {
                drive,
                source,
                kind,
            } if !self.running && !self.shell.command_active() => {
                let previous = self.shell.prompt();
                let result = match kind {
                    dosa_desktop::MountKind::SessionZip => Mount::open_session_zip(source),
                    dosa_desktop::MountKind::Iso => Mount::open_iso_image(source),
                    dosa_desktop::MountKind::Fat => Mount::open_fat_image(source),
                    _ => Mount::open(source),
                };
                self.status = match result
                    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
                    .and_then(|mount| self.shell.mount(drive, mount))
                {
                    Ok(()) => {
                        self.refresh_prompt(&previous);
                        format!("Mounted {drive}:")
                    }
                    Err(error) => format!("Mount failed: {error}"),
                };
            }
            MenuAction::Unmount(drive) if !self.running && !self.shell.command_active() => {
                let previous = self.shell.prompt();
                self.status = match self.shell.unmount(drive) {
                    Ok(()) => {
                        self.refresh_prompt(&previous);
                        format!("Unmounted {drive}:")
                    }
                    Err(error) => format!("Unmount failed: {error}"),
                };
            }
            MenuAction::SetCpu(mode) if !self.running => {
                if self
                    .shell
                    .machine
                    .set_cpu_profile(cpu_profile(mode))
                    .is_ok()
                {
                    self.settings.cpu = mode;
                }
            }
            MenuAction::SetSpeed(speed) => self.settings.speed = speed,
            MenuAction::SetVolume(volume) => self.volume = volume,
            MenuAction::SetMuted(muted) => self.muted = muted,
            MenuAction::StartRecording(path) if self.recording.is_none() => {
                match Recording::start(path) {
                    Ok(recording) => {
                        self.recording = Some(recording);
                        self.status.clear();
                    }
                    Err(error) => self.status = format!("WAV recording failed: {error}"),
                }
            }
            MenuAction::StopRecording => self.finish_recording(false),
            MenuAction::SetScaling(scaling) => self.scaling = scaling,
            MenuAction::SetPaused(paused) if self.running => {
                self.set_paused(paused);
            }
            MenuAction::SetInspection(inspecting) if self.running => {
                if inspecting {
                    self.set_paused(true);
                }
                self.inspecting = inspecting;
            }
            MenuAction::StepInstruction if self.running && self.paused => {
                self.inspecting = true;
                let result = self.shell.machine.step_paused();
                self.update_guest(result);
                self.render();
            }
            MenuAction::ResetGuest => self.reset_guest(),
            MenuAction::SetTextMode(mode) if !self.running => self.set_text_mode(mode),
            MenuAction::Exit => self.exit = true,
            _ => {}
        }
    }

    fn pointer(&mut self, event: PointerEvent) {
        match event {
            PointerEvent::Moved { x, y } if self.running && !self.paused => {
                self.shell.machine.mouse_move(x, y);
            }
            PointerEvent::Buttons { mask } if self.running && !self.paused => {
                self.shell.machine.mouse_buttons(mask);
            }
            PointerEvent::Deactivated => self.shell.machine.mouse_buttons(0),
            _ => {}
        }
    }

    fn pointer_mode(&self) -> PointerMode {
        if self.running && !self.paused && self.shell.machine.mouse_active() {
            PointerMode::Integrated
        } else {
            PointerMode::Host
        }
    }

    fn gamepads(&mut self, mut states: [Option<GamepadState>; 2]) {
        let first_connector = if self.settings.virtual_joystick {
            states[1] = states[0].or(states[1]);
            1
        } else {
            0
        };
        for (index, state) in states.into_iter().enumerate().skip(first_connector) {
            self.shell.machine.set_joystick(
                index,
                state.map(|state| {
                    if self.paused {
                        JoystickState::default()
                    } else {
                        JoystickState {
                            x: state.x,
                            y: state.y,
                            buttons: state.buttons,
                        }
                    }
                }),
            );
        }
    }

    fn input(&mut self, event: Input) {
        if self.running && self.joystick_input(event.scan_code, event.pressed) {
            return;
        }
        if !event.pressed {
            if self.running {
                self.shell.machine.key_up(event.scan_code);
            }
            return;
        }
        if self.running {
            if self.paused {
                return;
            }
            let ascii = match event.key {
                Key::Enter => 13,
                Key::Backspace => 8,
                Key::Tab => 9,
                Key::Escape => 27,
                Key::Character(character)
                    if event.modifiers.ctrl && character.is_ascii_alphabetic() =>
                {
                    character.to_ascii_uppercase() as u8 - b'@'
                }
                _ => event
                    .text
                    .as_deref()
                    .and_then(|text| text.chars().next())
                    .map_or(0, |character| {
                        self.shell.machine.code_page().encode(character)
                    }),
            };
            self.shell.machine.key(ascii, event.scan_code);
            return;
        }
        if self.shell.command_active() {
            if event.key != Key::Other && !event.modifiers.logo {
                let key = match event.key {
                    Key::Character('c' | 'C') if event.modifiers.ctrl => 3,
                    Key::Character('z' | 'Z') if event.modifiers.ctrl => 0x1a,
                    Key::Function(6) if self.shell.copy_text_waiting() => 0x1a,
                    Key::Character(character) if !event.modifiers.ctrl => {
                        self.shell.machine.code_page().encode(character)
                    }
                    Key::Enter => 13,
                    Key::Backspace => 8,
                    Key::Tab => 9,
                    Key::Escape => 27,
                    _ => 0,
                };
                if self.shell.input_waiting() {
                    let result = self.shell.resume_input(key);
                    match result {
                        Ok(None) => return,
                        Ok(Some(command)) => self.apply_command(Ok(command)),
                        Err(error) => {
                            self.shell.machine.write_console(b"\r\n");
                            self.apply_command(Err(error));
                        }
                    }
                    if !self.shell.command_active() {
                        self.finish_command();
                    }
                } else if let Some(response) = self.shell.resume_batch(key) {
                    self.shell.machine.write_console(response);
                    if !self.shell.command_active() {
                        self.finish_command();
                    }
                }
            }
            return;
        }
        match event.key {
            Key::Enter => {
                let line = std::mem::take(&mut self.line);
                self.submit(&line);
            }
            Key::Backspace => {
                if self.line.pop().is_some() {
                    self.shell.machine.erase_console_character();
                }
            }
            Key::Character(_) if !event.modifiers.ctrl && !event.modifiers.logo => {
                if let Some(text) = event.text {
                    for ch in text.chars().filter(|ch| !ch.is_control()) {
                        self.line.push(ch);
                        let byte = self.shell.machine.code_page().encode(ch);
                        self.shell.machine.write_console(&[byte]);
                    }
                }
            }
            _ => {}
        }
    }
}

fn save_screenshot(
    path: &std::path::Path,
    frame: Frame<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let mut encoder = png::Encoder::new(file.as_file_mut(), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(frame.rgba)?;
    writer.finish()?;
    file.persist(path)?;
    Ok(())
}

fn state_slot_directory() -> Option<PathBuf> {
    let absolute = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let root = if cfg!(windows) {
        absolute("LOCALAPPDATA").map(|path| path.join("Dosa"))
    } else if cfg!(target_os = "macos") {
        absolute("HOME").map(|path| path.join("Library/Application Support/Dosa"))
    } else {
        absolute("XDG_DATA_HOME")
            .or_else(|| absolute("HOME").map(|path| path.join(".local/share")))
            .map(|path| path.join("dosa"))
    };
    root.map(|path| path.join("states"))
}

fn slot_file(directory: &Path, slot: StateSlot) -> PathBuf {
    directory.join(format!("slot-{}.dosastate", slot.number()))
}

fn save_guest_file(shell: &Shell, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let state = shell.save_guest()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&state)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn load_guest_file(shell: &mut Shell, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_DESKTOP_SNAPSHOT_BYTES as u64 {
        return Err("save state file is too large".into());
    }
    let mut state = Vec::new();
    file.take(MAX_DESKTOP_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut state)?;
    if state.len() > MAX_DESKTOP_SNAPSHOT_BYTES {
        return Err("save state file is too large".into());
    }
    shell.load_guest(&state)?;
    Ok(())
}

fn cpu_profile(mode: CpuMode) -> CpuProfile {
    match mode {
        CpuMode::I8086 => CpuProfile::I8086,
        CpuMode::I286 => CpuProfile::I286,
        CpuMode::I386 => CpuProfile::I386,
    }
}

fn text_rows(mode: TextMode) -> u8 {
    match mode {
        TextMode::Rows25 | TextMode::WideRows25 => 25,
        TextMode::Rows43 | TextMode::WideRows43 => 43,
        TextMode::Rows50 | TextMode::WideRows50 => 50,
    }
}

fn text_columns(mode: TextMode) -> u16 {
    match mode {
        TextMode::WideRows25 | TextMode::WideRows43 | TextMode::WideRows50 => 132,
        _ => 80,
    }
}

pub fn instructions_per_tick(speed: Speed) -> usize {
    match speed {
        Speed::Slow => 10_000,
        Speed::Normal => 50_000,
        Speed::Fast => 100_000,
        Speed::Maximum => 250_000,
    }
}

fn draw_mouse_cursor(pixels: &mut [u8], width: u32, height: u32, cursor: &MouseCursor) {
    let origin_x = (u32::from(cursor.x) * (width - 1) / 65535) as i32 - i32::from(cursor.hotspot_x);
    let origin_y =
        (u32::from(cursor.y) * (height - 1) / 65535) as i32 - i32::from(cursor.hotspot_y);
    for (row, (&screen, &shape)) in cursor
        .screen_mask
        .iter()
        .zip(&cursor.cursor_mask)
        .enumerate()
    {
        let y = origin_y + row as i32;
        if !(0..height as i32).contains(&y) {
            continue;
        }
        for column in 0..16 {
            let x = origin_x + column;
            if !(0..width as i32).contains(&x) {
                continue;
            }
            let bit = 0x8000u16 >> column;
            let keep = if screen & bit != 0 { 255 } else { 0 };
            let invert = if shape & bit != 0 { 255 } else { 0 };
            let offset = ((y as u32 * width + x as u32) * 4) as usize;
            for channel in &mut pixels[offset..offset + 3] {
                *channel = (*channel & keep) ^ invert;
            }
        }
    }
}

fn screen_text(frame: TextFrame<'_>) -> String {
    let columns = usize::from(frame.columns);
    let cells = frame.cells.as_chunks::<2>().0;
    let mut text = String::new();
    for row in cells.chunks(columns) {
        let line: String = row
            .iter()
            .map(|cell| {
                if cell[0] == 0 {
                    ' '
                } else {
                    frame.code_page.character(cell[0])
                }
            })
            .collect();
        text.push_str(line.trim_end_matches(' '));
        text.push('\n');
    }
    text.truncate(text.trim_end_matches('\n').len());
    text
}

fn paste_characters(text: &str) -> impl Iterator<Item = char> + '_ {
    let mut previous_cr = false;
    text.chars().filter_map(move |character| {
        let result = match character {
            '\n' if previous_cr => None,
            '\r' | '\n' => Some('\r'),
            '\t' => Some(character),
            character if !character.is_control() => Some(character),
            _ => None,
        };
        previous_cr = character == '\r';
        result
    })
}

fn render_text(frame: TextFrame<'_>, width: usize, pixels: &mut [u8], blink: bool) {
    let columns = usize::from(frame.columns);
    let height = usize::from(frame.character_height);
    let glyphs = frame.code_page.glyphs();
    for (cell, data) in frame.cells.as_chunks::<2>().0.iter().enumerate() {
        let (column, row) = (cell % columns, cell / columns);
        let cursor = blink && frame.cursor == Some((column as u16, row as u16));
        let glyph = glyphs[usize::from(data[0])];
        let color = VGA[usize::from(data[1] & 0x0f)];
        let background = VGA[usize::from(data[1] >> 4 & 0x07)];
        let visible = data[1] & 0x80 == 0 || blink;
        for y in 0..height {
            let start = ((row * height + y) * width + column * 8) * 4;
            for (x, pixel) in pixels[start..start + 32]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                let [r, g, b] = if cursor && y + 2 >= height
                    || visible && glyph[y * 8 / height] & (1 << x) != 0
                {
                    color
                } else {
                    background
                };
                pixel.copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_copy_decodes_cp437_and_shell_paste_stays_editable() {
        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.shell.machine.clear_console();
        desktop
            .shell
            .machine
            .write_console(&[0xda, 0x82, b' ', b'\r', b'\n', b'\r', b'\n', b'X']);
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected text mode")
        };
        assert_eq!(screen_text(frame), "┌é\n\nX");

        desktop.paste_text("echo ok\r\nexit\t\u{1b}");
        assert_eq!(desktop.line, "echo ok exit ");
        assert!(!desktop.exit);
    }

    #[test]
    fn copy_con_desktop_finishes_with_f6_and_enter() {
        let directory = tempfile::tempdir().unwrap();
        let mut shell = Shell::new();
        shell
            .mount(
                'C',
                Mount::open_writable_directory(directory.path()).unwrap(),
            )
            .unwrap();
        let mut desktop = Desktop::new(shell, None);
        desktop.submit("COPY CON NOTE.TXT");
        assert!(desktop.shell.input_waiting());
        for (key, text) in [
            (Key::Character('a'), Some("a".to_owned())),
            (Key::Enter, None),
            (Key::Function(6), None),
            (Key::Enter, None),
        ] {
            desktop.input(Input {
                key,
                text,
                scan_code: 0,
                pressed: true,
                modifiers: Default::default(),
            });
        }
        assert!(!desktop.shell.input_waiting());
        assert_eq!(
            std::fs::read(directory.path().join("NOTE.TXT")).unwrap(),
            b"a\r\n"
        );
    }

    #[test]
    fn chcp_changes_text_rendering_and_paste_encoding() {
        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.shell.machine.clear_console();
        desktop.shell.machine.write_console(&[0x9b]);
        let rendered = |desktop: &Desktop, expected: &str| {
            let Video::Text(frame) = desktop.shell.machine.video() else {
                panic!("expected text mode")
            };
            assert_eq!(screen_text(frame), expected);
            let width = usize::from(frame.columns) * 8;
            let mut pixels = vec![0; width * usize::from(frame.rows * frame.character_height) * 4];
            render_text(frame, width, &mut pixels, false);
            pixels
        };

        let original = rendered(&desktop, "\u{a2}");
        desktop.shell.execute("CHCP 850").unwrap();
        let switched = rendered(&desktop, "\u{f8}");
        assert_ne!(switched, original);
        desktop.running = true;
        desktop.paste_text("\u{f8}");
        assert_eq!(desktop.paste.pop_front(), Some(0x9b));
    }

    #[test]
    fn gamepad_updates_and_disconnects_reach_the_dos_gameport() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("JOY.COM"),
            [0xba, 1, 2, 0xec, 0xb4, 0x4c, 0xcd, 0x21],
        )
        .unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(folder.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::with_settings(
            shell,
            None,
            Settings {
                virtual_joystick: true,
                ..Settings::default()
            },
        )
        .unwrap();
        let pad = GamepadState {
            x: 50,
            buttons: [false, true],
            ..Default::default()
        };
        for (states, expected) in [
            ([Some(pad), None], 0x70),
            ([None, Some(pad)], 0x70),
            ([None, None], 0xfc),
        ] {
            desktop.submit("JOY");
            desktop.gamepads(states);
            desktop.tick();
            assert_eq!(desktop.shell.errorlevel(), expected);
        }
    }

    #[test]
    fn virtual_joystick_keyboard_input_reaches_the_guest_and_centers_on_pause() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("JOY.COM"),
            [
                0xba, 1, 2, 0xec, 0x88, 0xc3, // BL = gameport byte
                0xb4, 1, 0xcd, 0x16, 0x74, 2, // No keyboard input should leak through
                0xb3, 1, 0x88, 0xd8, 0xb4, 0x4c, 0xcd, 0x21, // Exit with BL
            ],
        )
        .unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(folder.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::with_settings(
            shell,
            None,
            Settings {
                virtual_joystick: true,
                ..Settings::default()
            },
        )
        .unwrap();
        let input = |scan_code, pressed| Input {
            key: Key::Other,
            scan_code,
            pressed,
            text: None,
            modifiers: Default::default(),
        };
        desktop.submit("JOY");
        desktop.input(input(0x4b, true));
        desktop.input(input(0x2c, true));
        desktop.input(input(0x2d, true));
        desktop.input(input(0x2d, false));
        assert_eq!(
            desktop.joystick_keys,
            [true, false, false, false, true, false]
        );
        desktop.tick();
        assert_eq!(desktop.shell.errorlevel(), 0xec);
        assert_eq!(desktop.joystick_keys, [false; 6]);

        desktop.submit("JOY");
        desktop.input(input(0x2c, true));
        desktop.menu_action(MenuAction::SetPaused(true));
        desktop.input(input(0x2d, true));
        desktop.menu_action(MenuAction::SetPaused(false));
        desktop.tick();
        assert_eq!(desktop.shell.errorlevel(), 0xfc);
    }

    #[test]
    fn ansi_sequences_reach_the_desktop_from_shell_and_guest_output() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("COLOR.TXT"),
            b"\x1b[2J\x1b[2;4H\x1b[1;31;44mX\x1b[s\x1b[8;1H\x1b[0mY\x1b[u",
        )
        .unwrap();
        let mut guest = vec![
            0xba, 0x20, 1, 0xb4, 9, 0xcd, 0x21, // Write ESC [ in one DOS call.
            0xba, 0x23, 1, 0xb4, 9, 0xcd, 0x21, // Complete the sequence in the next call.
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ];
        guest.resize(0x20, 0);
        guest.extend_from_slice(b"\x1b[$3;7H\x1b[32mZ\x1b[0m$");
        std::fs::write(folder.path().join("GUEST.COM"), guest).unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open_writable_directory(folder.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::with_settings(
            shell,
            None,
            Settings {
                ansi_text: true,
                ..Settings::default()
            },
        )
        .unwrap();
        desktop.submit("TYPE COLOR.TXT");
        desktop.submit("TYPE COLOR.TXT > COPY.TXT");
        desktop.submit("GUEST");
        desktop.tick();

        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected text mode");
        };
        let cells = frame.cells.as_chunks::<2>().0;
        assert_eq!(cells[80 + 3], [b'X', 0x1c]);
        assert_eq!(cells[160 + 6], [b'Z', 0x02]);
        assert_eq!(cells[560], [b'Y', 0x07]);
        assert_eq!(
            std::fs::read(folder.path().join("COPY.TXT")).unwrap(),
            std::fs::read(folder.path().join("COLOR.TXT")).unwrap()
        );
    }

    #[test]
    fn guest_paste_waits_for_existing_keys_and_tears_down_on_exit() {
        let folder = tempfile::tempdir().unwrap();
        let mut program = Vec::new();
        for _ in 0..4 {
            program.extend([0xb4, 0, 0xcd, 0x16, 0x88, 0xc2, 0xb4, 2, 0xcd, 0x21]);
        }
        program.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
        std::fs::write(folder.path().join("READ.COM"), program).unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(folder.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("read"));
        desktop.shell.machine.key(b'X', 0);
        desktop.paste_text(&"x".repeat(MAX_PASTE_CHARS + 20));
        assert_eq!(desktop.paste.len(), MAX_PASTE_CHARS);
        desktop.menu_action(MenuAction::StopPaste);
        assert!(desktop.paste.is_empty());
        desktop.paste_text("A\r\néremaining");
        desktop.tick();
        assert_eq!(desktop.paste.front(), Some(&b'A'));
        desktop.tick();
        assert_eq!(desktop.paste.front(), Some(&b'\r'));
        desktop.tick();
        assert_eq!(desktop.paste.front(), Some(&0x82));
        desktop.tick();
        assert!(!desktop.running);
        assert!(desktop.paste.is_empty());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected text mode")
        };
        assert!(screen_text(frame).contains("éA")); // Carriage return overwrites the first column.
    }

    #[test]
    fn quick_launch_runs_a_nested_batch_and_restores_the_shell() {
        let programs = tempfile::tempdir().unwrap();
        let batch = programs.path().join("Launch some DOS tools.bat");
        std::fs::write(&batch, b"@echo off\ncall nested.bat\ncd C:\\\n").unwrap();
        std::fs::write(
            programs.path().join("nested.bat"),
            b"@echo off\nchild.com\nif errorlevel 7 type result.txt\n",
        )
        .unwrap();
        std::fs::write(
            programs.path().join("child.com"),
            [0xb8, 7, 0x4c, 0xcd, 0x21],
        )
        .unwrap();
        std::fs::write(programs.path().join("result.txt"), b"sibling available\r\n").unwrap();
        let existing = tempfile::tempdir().unwrap();
        std::fs::create_dir(existing.path().join("SUB")).unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(existing.path()).unwrap())
            .unwrap();
        shell.execute("CD SUB").unwrap();
        let previous = shell.prompt();
        let mut desktop = Desktop::new(shell, None);
        desktop.menu_action(MenuAction::QuickLaunch(batch));
        assert!(desktop.shell.batch_active());
        assert_eq!(desktop.menu().active_drive, 'Z');
        assert_eq!(desktop.menu().drives[&'Z'], "Folder (read-only)");
        for _ in 0..4 {
            desktop.tick();
        }
        assert!(!desktop.running && !desktop.shell.batch_active());
        assert_eq!(desktop.shell.prompt(), previous);
        assert_eq!(
            desktop.menu().drives.keys().copied().collect::<Vec<_>>(),
            ['C']
        );
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected text mode")
        };
        let text: Vec<_> = frame
            .cells
            .as_chunks::<2>()
            .0
            .iter()
            .map(|cell| cell[0])
            .collect();
        assert!(
            text.windows(b"sibling available".len())
                .any(|bytes| bytes == b"sibling available")
        );
    }

    #[test]
    fn quick_launch_cleans_up_after_a_guest_instruction_error() {
        let programs = tempfile::tempdir().unwrap();
        let program = programs.path().join("oops.COM");
        std::fs::write(&program, [0x0f, 0x0b]).unwrap();
        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.menu_action(MenuAction::QuickLaunch(program));
        assert!(desktop.running);
        desktop.tick();
        assert!(!desktop.running);
        assert!(desktop.menu().drives.is_empty());
        assert_eq!(desktop.menu().active_drive, 'C');
    }

    #[test]
    fn desktop_unmount_falls_back_to_remaining_drive_and_allows_remount() {
        let folder = tempfile::tempdir().unwrap();
        let mut shell = Shell::new();
        shell
            .mount('d', Mount::open(folder.path()).unwrap())
            .unwrap();
        shell
            .mount('e', Mount::open_writable_directory(folder.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, None);
        assert_eq!(desktop.menu().drives[&'D'], "Folder (read-only)");
        assert_eq!(desktop.menu().drives[&'E'], "Folder (read/write)");
        desktop.input(Input {
            key: Key::Character('d'),
            pressed: true,
            text: Some("dir".into()),
            modifiers: Default::default(),
            scan_code: 0,
        });
        desktop.menu_action(MenuAction::Unmount('D'));
        assert_eq!(desktop.menu().active_drive, 'E');
        assert!(!desktop.menu().drives.contains_key(&'D'));
        assert_eq!(desktop.line, "dir");
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected text mode")
        };
        let (column, row) = frame.cursor.unwrap();
        let expected = b"E:\\> dir";
        let start = usize::from(row) * usize::from(frame.columns) * 2;
        assert_eq!(usize::from(column), expected.len());
        assert!(
            expected
                .iter()
                .enumerate()
                .all(|(index, byte)| frame.cells[start + index * 2] == *byte)
        );
        desktop.menu_action(MenuAction::Unmount('E'));
        assert!(desktop.menu().drives.is_empty());
        desktop.menu_action(MenuAction::Mount {
            drive: 'D',
            source: folder.path().into(),
            kind: dosa_desktop::MountKind::Folder,
        });
        assert_eq!(desktop.menu().active_drive, 'D');
        assert!(desktop.shell.execute("DIR").is_ok());
    }

    #[test]
    fn screenshot_round_trips_guest_rgba_pixels() {
        let path = std::env::temp_dir().join(format!("dosa-capture-{}.png", std::process::id()));
        let rgba = [10, 20, 30, 255, 40, 50, 60, 255];
        save_screenshot(
            &path,
            Frame {
                width: 2,
                height: 1,
                rgba: &rgba,
            },
        )
        .unwrap();
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&decoded[..info.buffer_size()], &rgba);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_capture_preserves_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.png");
        std::fs::write(&path, b"existing image").unwrap();
        assert!(
            save_screenshot(
                &path,
                Frame {
                    width: 2,
                    height: 1,
                    rgba: &[0]
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"existing image");
    }

    #[test]
    fn reset_stops_a_paused_batch_and_preserves_mounts_and_settings() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(
            scratch.path().join("RUN.BAT"),
            b"@echo off\r\nWAIT\r\necho BATCH-CONTINUED\r\n",
        )
        .unwrap();
        std::fs::write(
            scratch.path().join("WAIT.COM"),
            [0xb4, 0, 0xcd, 0x16, 0xb8, 0, 0x4c, 0xcd, 0x21],
        )
        .unwrap();
        let mut shell = Shell::new();
        shell
            .mount('D', Mount::open_writable_directory(scratch.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("run"));
        desktop.tick();
        desktop.tick();
        assert!(desktop.menu().running);
        assert!(desktop.menu().batch_active);
        desktop.menu_action(MenuAction::SetInspection(true));
        assert!(desktop.menu().paused);
        desktop.menu_action(MenuAction::SetSpeed(Speed::Fast));
        desktop.menu_action(MenuAction::ResetGuest);
        desktop.tick();

        let menu = desktop.menu();
        assert!(!menu.running && !menu.paused && !menu.batch_active);
        assert!(menu.inspection.is_empty());
        assert_eq!(menu.active_drive, 'D');
        assert_eq!(menu.drives.get(&'D'), Some(&"Folder (read/write)"));
        assert_eq!(menu.speed, Speed::Fast);
        assert_eq!(menu.status, "Guest reset");
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("reset should show the prompt")
        };
        let text = screen_text(frame);
        assert!(text.contains("D:\\>"));
        assert!(!text.contains("BATCH-CONTINUED"));
    }

    #[test]
    fn menu_selects_profiles_speed_and_text_geometry() {
        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.menu_action(MenuAction::SetCpu(CpuMode::I286));
        desktop.menu_action(MenuAction::SetSpeed(Speed::Fast));
        desktop.menu_action(MenuAction::SetTextMode(TextMode::Rows43));
        desktop.tick();

        let menu = desktop.menu();
        assert_eq!(menu.cpu, CpuMode::I286);
        assert_eq!(menu.speed, Speed::Fast);
        assert_eq!(menu.text_mode, TextMode::Rows43);
        assert_eq!((desktop.frame().width, desktop.frame().height), (640, 344));
        assert!(instructions_per_tick(Speed::Fast) > instructions_per_tick(Speed::Normal));
        desktop.menu_action(MenuAction::SetTextMode(TextMode::WideRows50));
        desktop.tick();
        assert_eq!((desktop.frame().width, desktop.frame().height), (1056, 400));
    }

    #[test]
    fn running_guest_can_recover_from_a_read_only_zip_write_denial() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("programs.zip");
        let mut guest = vec![
            0xb8, 0x13, 0, 0xcd, 0x10, // VGA mode 13h
            0xba, 0x20, 1, 0xb8, 2, 0x3d, 0xcd, 0x21, // Open read/write
            0x73, 2, 0xeb, 0xfe, // Stay on a black screen on carry
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ];
        guest.resize(0x20, 0);
        guest.extend_from_slice(b"STATE.DAT\0");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("RUN.COM", options).unwrap();
        zip.write_all(&guest).unwrap();
        zip.start_file("STATE.DAT", options).unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap();
        let original = std::fs::read(&path).unwrap();

        let mut shell = Shell::new();
        shell.mount('C', Mount::open(&path).unwrap()).unwrap();
        let mut desktop = Desktop::new(shell, Some("run"));
        desktop.tick();
        assert!(desktop.guest_graphics_active());
        assert_eq!(desktop.menu().write_blocked_zip, Some('C'));
        assert_eq!(desktop.menu().status, "C: write denied. See Drives.");
        desktop.menu_action(MenuAction::UseWritableZip('C'));
        assert!(!desktop.menu().running);
        assert_eq!(desktop.menu().write_blocked_zip, None);
        assert_eq!(
            desktop.menu().drives.get(&'C'),
            Some(&"ZIP copy (session R/W)")
        );
        desktop.submit("run");
        desktop.tick();
        assert!(!desktop.menu().running);
        assert_eq!(desktop.menu().write_blocked_zip, None);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn audio_actions_keep_the_selected_volume_when_muted() {
        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.menu_action(MenuAction::SetVolume(Volume::Half));
        desktop.menu_action(MenuAction::SetMuted(true));
        assert!(desktop.menu().muted);
        assert_eq!(desktop.menu().volume, Volume::Half);
        desktop.menu_action(MenuAction::SetMuted(false));
        assert!(!desktop.menu().muted);
        assert_eq!(desktop.menu().volume, Volume::Half);
    }

    #[test]
    fn wav_recording_captures_muted_guest_audio_and_saves_on_stop_and_close() {
        let scratch = tempfile::tempdir().unwrap();
        let guest = [
            0xba, 0x2c, 0x02, 0xb0, 0xd1, 0xee, // Enable Sound Blaster DAC
            0xb0, 0x10, 0xee, 0xb0, 64, 0xee, // Set a known DAC level
            0x31, 0xc0, 0xcd, 0x16, // Wait for a key
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ];
        std::fs::write(scratch.path().join("TONE.COM"), guest).unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(scratch.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("tone"));
        desktop.menu_action(MenuAction::SetMuted(true));
        let path = scratch.path().join("stopped.wav");
        desktop.menu_action(MenuAction::StartRecording(path.clone()));
        assert!(desktop.menu().recording);
        desktop.tick();
        let first = desktop.audio();
        std::thread::sleep(std::time::Duration::from_millis(20));
        desktop.tick();
        let tone = desktop.audio();
        assert!(tone.contains(&-0.25));
        desktop.menu_action(MenuAction::SetPaused(true));
        desktop.tick();
        assert!(desktop.audio().is_empty());
        desktop.menu_action(MenuAction::StopRecording);
        assert!(!desktop.menu().recording);
        assert_eq!(desktop.menu().status, "WAV saved");

        let wav = std::fs::read(path).unwrap();
        let data_bytes = ((first.len() + tone.len()) * 2) as u32;
        assert_eq!(wav.len(), 44 + data_bytes as usize);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[4..8], &(36 + data_bytes).to_le_bytes());
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(&wav[20..24], &[1, 0, 1, 0]);
        assert_eq!(&wav[24..28], &48_000u32.to_le_bytes());
        assert_eq!(&wav[34..40], b"\x10\0data");
        assert_eq!(&wav[40..44], &data_bytes.to_le_bytes());
        assert!(
            wav[44..]
                .as_chunks::<2>()
                .0
                .contains(&(-8192i16).to_le_bytes())
        );

        let path = scratch.path().join("closed.wav");
        desktop.menu_action(MenuAction::StartRecording(path.clone()));
        assert!(!path.exists());
        drop(desktop);
        assert_eq!(std::fs::read(path).unwrap().len(), 44);
    }

    #[test]
    fn mouse_overlay_clips_top_left_and_keeps_alpha() {
        let mut pixels = [10, 20, 30, 255, 40, 50, 60, 255];
        let mut cursor = MouseCursor {
            x: 0,
            y: 0,
            hotspot_x: 1,
            hotspot_y: 1,
            screen_mask: [u16::MAX; 16],
            cursor_mask: [0; 16],
        };
        cursor.cursor_mask[1] = 0x4000;
        draw_mouse_cursor(&mut pixels, 2, 1, &cursor);
        assert_eq!(pixels, [245, 235, 225, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn desktop_date_prompt_accepts_editing_and_returns_to_the_shell() {
        let mut desktop = Desktop::new(Shell::new(), Some("DATE"));
        let mut press = |key| {
            desktop.input(Input {
                key,
                pressed: true,
                text: None,
                modifiers: Default::default(),
                scan_code: 0,
            })
        };
        press(Key::Character('3'));
        press(Key::Backspace);
        for character in "02-29-2000".chars() {
            press(Key::Character(character));
        }
        press(Key::Enter);

        assert!(!desktop.shell.input_waiting());
        assert_eq!(
            desktop.shell.machine.clock_snapshot().date(),
            (2000, 2, 29, 2)
        );
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        let text = screen_text(frame);
        assert!(
            text.contains("Enter new date (mm-dd-yy): 02-29-2000"),
            "{text}"
        );
        assert!(text.trim_end().ends_with("C:\\>"));
    }

    #[test]
    fn desktop_dir_p_waits_for_a_key_then_returns_to_the_prompt() {
        let scratch = tempfile::tempdir().unwrap();
        for index in 0..30 {
            std::fs::write(scratch.path().join(format!("F{index:02}.TXT")), b"x").unwrap();
        }
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(scratch.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("DIR/P/B"));
        desktop.tick();
        assert!(desktop.shell.input_waiting());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        let text = screen_text(frame);
        assert!(text.contains("Press any key to continue"));
        assert!(!text.contains("F24.TXT"));
        desktop.input(Input {
            key: Key::Enter,
            pressed: true,
            text: None,
            modifiers: Default::default(),
            scan_code: 0x1c,
        });
        assert!(!desktop.shell.input_waiting());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        let text = screen_text(frame);
        assert!(text.contains("F29.TXT"));
        assert!(text.trim_end().ends_with("C:\\>"));
    }

    #[test]
    fn desktop_delete_confirmation_waits_for_input_and_returns_to_the_prompt() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("TEST.TXT");
        std::fs::write(&file, b"test").unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open_writable_directory(scratch.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("del TEST.TXT /p"));
        desktop.tick();
        assert!(desktop.shell.input_waiting());
        assert!(file.exists());
        desktop.input(Input {
            key: Key::Character('y'),
            pressed: true,
            text: Some("y".into()),
            modifiers: Default::default(),
            scan_code: 0x15,
        });
        assert!(!desktop.shell.input_waiting());
        assert!(!file.exists());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        let text = screen_text(frame);
        assert!(text.contains("Delete C:\\TEST.TXT (Y/N)? Y"));
        assert!(text.trim_end().ends_with("C:\\>"));
    }

    #[test]
    fn desktop_batch_pause_waits_across_ticks_before_resuming() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(
            scratch.path().join("WAIT.BAT"),
            b"@echo off\r\npause\r\necho resumed\r\n",
        )
        .unwrap();
        let mut shell = Shell::new();
        shell
            .mount('C', Mount::open(scratch.path()).unwrap())
            .unwrap();
        let mut desktop = Desktop::new(shell, Some("wait"));
        desktop.tick();
        desktop.tick();
        assert!(desktop.shell.batch_waiting());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        assert!(screen_text(frame).contains("Press any key to continue"));
        assert!(!screen_text(frame).contains("resumed"));
        desktop.input(Input {
            key: Key::Enter,
            pressed: true,
            text: None,
            modifiers: Default::default(),
            scan_code: 0x1c,
        });
        desktop.tick();
        assert!(!desktop.shell.batch_active());
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        assert!(screen_text(frame).contains("resumed"));
    }

    #[test]
    fn desktop_batch_waits_for_guest_input_and_returns_to_the_prompt() {
        let scratch = std::env::temp_dir().join(format!("dosa-batch-{}", std::process::id()));
        std::fs::create_dir(&scratch).unwrap();
        let script = scratch.join("RUN.BAT");
        let guest = scratch.join("WAITKEY.COM");
        std::fs::write(
            &script,
            b"@echo off\r\nwaitkey\r\necho resumed %errorlevel%\r\n",
        )
        .unwrap();
        std::fs::write(&guest, [0x31, 0xc0, 0xcd, 0x16, 0xb8, 9, 0x4c, 0xcd, 0x21]).unwrap();

        let mut shell = Shell::new();
        shell.mount('C', Mount::open(&scratch).unwrap()).unwrap();
        let mut desktop = Desktop::new(shell, Some("run"));
        desktop.tick();
        desktop.tick();
        assert!(desktop.running && desktop.shell.batch_active());
        assert_eq!(desktop.shell.errorlevel(), 0);

        desktop.input(Input {
            key: Key::Enter,
            pressed: true,
            text: None,
            modifiers: Default::default(),
            scan_code: 0x1c,
        });
        desktop.tick();
        assert!(!desktop.running && !desktop.shell.batch_active());
        assert_eq!(desktop.shell.errorlevel(), 9);
        let Video::Text(frame) = desktop.shell.machine.video() else {
            panic!("expected the text console");
        };
        let chars: Vec<u8> = frame.cells.iter().step_by(2).copied().collect();
        assert!(
            chars
                .windows(b"resumed 9".len())
                .any(|word| word == b"resumed 9")
        );
        let (col, row) = frame.cursor.unwrap();
        assert_eq!(col, 5);
        let cursor_line = usize::from(row) * usize::from(frame.columns);
        assert_eq!(&chars[cursor_line..cursor_line + 5], b"C:\\> ");

        drop(desktop);
        std::fs::remove_file(script).unwrap();
        std::fs::remove_file(guest).unwrap();
        std::fs::remove_dir(scratch).unwrap();
    }

    #[test]
    fn desktop_save_and_load_restores_a_paused_guest() {
        let media = tempfile::tempdir().unwrap();
        let states = tempfile::tempdir().unwrap();
        let path = states.path().join("checkpoint.dosastate");
        let damaged = states.path().join("damaged.dosastate");
        std::fs::write(
            media.path().join("STATE.COM"),
            [
                0xb4, 0, 0xcd, 0x16, // Wait for a key.
                0xb4, 0x4c, 0xcd, 0x21, // Return the key as the exit code.
            ],
        )
        .unwrap();
        let desktop = || {
            let mut shell = Shell::new();
            shell
                .mount('C', Mount::open(media.path()).unwrap())
                .unwrap();
            Desktop::new(shell, None)
        };

        let mut original = desktop();
        original.menu_action(MenuAction::SetCpu(CpuMode::I286));
        original.submit("STATE");
        original.tick();
        original.menu_action(MenuAction::SetPaused(true));
        original.menu_action(MenuAction::SaveGuestState(path.clone()));
        assert_eq!(original.menu().status, "State saved");

        let mut restored = desktop();
        restored.menu_action(MenuAction::LoadGuestState(path));
        let menu = restored.menu();
        assert!(menu.running && menu.paused);
        assert_eq!(menu.cpu, CpuMode::I286);
        assert_eq!(menu.status, "State loaded");

        std::fs::write(&damaged, b"invalid file").unwrap();
        restored.menu_action(MenuAction::LoadGuestState(damaged));
        assert!(restored.menu().status.starts_with("Load state failed"));
        assert!(restored.menu().running && restored.menu().paused);

        restored.menu_action(MenuAction::SetPaused(false));
        restored.shell.machine.key(b'Y', 0x15);
        restored.tick();
        assert!(!restored.menu().running);
        assert_eq!(restored.shell.errorlevel(), b'Y');
    }

    #[test]
    fn desktop_restores_a_guest_and_resumes_its_nested_batch_loop() {
        let media = tempfile::tempdir().unwrap();
        let states = tempfile::tempdir().unwrap();
        let snapshot = states.path().join("batch.dosastate");
        std::fs::write(
            media.path().join("WAIT.COM"),
            [0xb4, 0, 0xcd, 0x16, 0xb4, 0x4c, 0xcd, 0x21],
        )
        .unwrap();
        std::fs::write(
            media.path().join("OUTER.BAT"),
            b"@ECHO OFF\r\nFOR %%I IN (FIRST SECOND) DO CALL CHILD %%I\r\nECHO OUTER-DONE\r\n",
        )
        .unwrap();
        std::fs::write(
            media.path().join("CHILD.BAT"),
            b"IF \"%1\"==\"FIRST\" WAIT.COM\r\nGOTO RESUME\r\nECHO SKIPPED\r\n:RESUME\r\nECHO CHILD-%1\r\n",
        ).unwrap();
        let desktop = |command: Option<&str>| {
            let mut shell = Shell::new();
            shell
                .mount('C', Mount::open(media.path()).unwrap())
                .unwrap();
            Desktop::new(shell, command)
        };
        let mut original = desktop(Some("OUTER"));
        for _ in 0..3 {
            original.tick();
        }
        assert!(original.menu().running && original.menu().batch_active);
        original.menu_action(MenuAction::SetPaused(true));
        original.menu_action(MenuAction::SaveGuestState(snapshot.clone()));
        assert_eq!(original.menu().status, "State saved");

        let mut restored = desktop(None);
        restored.menu_action(MenuAction::LoadGuestState(snapshot));
        assert!(restored.menu().running && restored.menu().paused && restored.menu().batch_active);
        restored.menu_action(MenuAction::SetPaused(false));
        restored.shell.machine.key(b'Y', 0x15);
        for _ in 0..3 {
            restored.tick();
        }
        assert!(!restored.menu().running && !restored.menu().batch_active);
        let Video::Text(frame) = restored.shell.machine.video() else {
            panic!("expected the text console");
        };
        let text = screen_text(frame);
        assert!(
            text.contains("CHILD-FIRST\nCHILD-SECOND\nOUTER-DONE"),
            "{text}"
        );
        assert!(!text.contains("SKIPPED"), "{text}");
    }

    #[test]
    fn desktop_restores_a_shared_batch_output_position() {
        let media = tempfile::tempdir().unwrap();
        let states = tempfile::tempdir().unwrap();
        let snapshot = states.path().join("redirected.dosastate");
        let result = media.path().join("RESULT.TXT");
        let mut program = vec![
            0xba, 0x20, 0x01, 0xb4, 9, 0xcd, 0x21, // Write BEFORE.
            0x30, 0xe4, 0xcd, 0x16, // Wait for a key.
            0xba, 0x27, 0x01, 0xb4, 9, 0xcd, 0x21, // Write AFTER.
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ];
        program.resize(0x20, 0);
        program.extend(b"BEFORE$AFTER$");
        std::fs::write(media.path().join("WAIT.COM"), program).unwrap();
        std::fs::write(
            media.path().join("OUTER.BAT"),
            b"@ECHO OFF\r\nCALL CHILD >RESULT.TXT\r\n",
        )
        .unwrap();
        std::fs::write(
            media.path().join("CHILD.BAT"),
            b"WAIT.COM\r\nECHO FINAL\r\n",
        )
        .unwrap();
        let desktop = |command: Option<&str>| {
            let mut shell = Shell::new();
            shell
                .mount('C', Mount::open_writable_directory(media.path()).unwrap())
                .unwrap();
            Desktop::new(shell, command)
        };
        let mut original = desktop(Some("OUTER"));
        for _ in 0..3 {
            original.tick();
        }
        assert!(original.menu().running && original.menu().batch_active);
        assert_eq!(std::fs::read(&result).unwrap(), b"BEFORE");
        original.menu_action(MenuAction::SetPaused(true));
        original.menu_action(MenuAction::SaveGuestState(snapshot.clone()));
        assert_eq!(original.menu().status, "State saved");

        let mut restored = desktop(None);
        restored.menu_action(MenuAction::LoadGuestState(snapshot));
        assert_eq!(restored.menu().status, "State loaded");
        assert_eq!(std::fs::read(&result).unwrap(), b"BEFORE");
        restored.menu_action(MenuAction::SetPaused(false));
        restored.shell.machine.key(b'Y', 0x15);
        for _ in 0..3 {
            restored.tick();
        }
        assert!(!restored.menu().running && !restored.menu().batch_active);
        assert_eq!(std::fs::read(result).unwrap(), b"BEFOREAFTERFINAL\r\n");
    }

    #[test]
    fn desktop_restores_a_pipeline_during_production_and_consumption() {
        let media = tempfile::tempdir().unwrap();
        let states = tempfile::tempdir().unwrap();
        let producer_state = states.path().join("producer.dosastate");
        let consumer_state = states.path().join("consumer.dosastate");
        let result = media.path().join("RESULT.TXT");
        let mut producer = vec![
            0xba, 0x20, 1, 0xb4, 9, 0xcd, 0x21, // Write BEFORE.
            0x30, 0xe4, 0xcd, 0x16, // Wait for the checkpoint.
            0xba, 0x27, 1, 0xb4, 9, 0xcd, 0x21, // Write AFTER.
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ];
        producer.resize(0x20, 0);
        producer.extend(b"BEFORE$AFTER$");
        let transfer = |length| {
            [
                0x31, 0xdb, 0xb9, length, 0, 0xba, 0, 2, 0xb4, 0x3f, 0xcd,
                0x21, // Read stdin.
                0x89, 0xc1, 0xbb, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // Write to stdout.
            ]
        };
        let consumer = [
            transfer(2).as_slice(),
            &[0x30, 0xe4, 0xcd, 0x16], // Wait while positioned within the pipe.
            transfer(64).as_slice(),
            &[0xb8, 0, 0x4c, 0xcd, 0x21],
        ]
        .concat();
        std::fs::write(media.path().join("WRITE.COM"), producer).unwrap();
        std::fs::write(media.path().join("READ.COM"), consumer).unwrap();
        let desktop = |command: Option<&str>| {
            let mut shell = Shell::new();
            shell
                .mount('C', Mount::open_writable_directory(media.path()).unwrap())
                .unwrap();
            Desktop::new(shell, command)
        };
        let mut writer = desktop(Some("WRITE | READ > RESULT.TXT"));
        writer.tick();
        assert!(writer.menu().running && writer.menu().batch_active);
        writer.menu_action(MenuAction::SaveGuestState(producer_state.clone()));
        assert_eq!(writer.menu().status, "State saved");

        let mut reader = desktop(None);
        reader.menu_action(MenuAction::LoadGuestState(producer_state));
        assert_eq!(reader.menu().status, "State loaded");
        reader.shell.machine.key(b'Y', 0x15);
        for _ in 0..3 {
            reader.tick();
        }
        assert_eq!(std::fs::read(&result).unwrap(), b"BE");
        reader.menu_action(MenuAction::SaveGuestState(consumer_state.clone()));
        assert_eq!(reader.menu().status, "State saved");

        let mut restored = desktop(None);
        restored.menu_action(MenuAction::LoadGuestState(consumer_state));
        assert_eq!(restored.menu().status, "State loaded");
        restored.shell.machine.key(b'Y', 0x15);
        for _ in 0..3 {
            restored.tick();
        }
        assert!(!restored.menu().running && !restored.menu().batch_active);
        assert_eq!(std::fs::read(result).unwrap(), b"BEFOREAFTER");
    }

    #[test]
    fn desktop_state_slots_replace_one_checkpoint_and_preserve_the_others() {
        let media = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let slots = storage.path().join("states");
        std::fs::write(
            media.path().join("WAIT.COM"),
            [0xb4, 0, 0xcd, 0x16, 0xb4, 0x4c, 0xcd, 0x21],
        )
        .unwrap();
        let desktop = |command: Option<&str>| {
            let mut shell = Shell::new();
            shell
                .mount('C', Mount::open(media.path()).unwrap())
                .unwrap();
            let mut desktop = Desktop::new(shell, command);
            desktop.state_slots = Some(slots.clone());
            desktop
        };
        let mut original = desktop(Some("WAIT.COM"));
        original.tick();
        original.menu_action(MenuAction::SetPaused(true));
        original.menu_action(MenuAction::SaveStateSlot(StateSlot::One));
        assert_eq!(original.menu().status, "Slot 1 saved");
        original.menu_action(MenuAction::SaveStateSlot(StateSlot::Two));
        assert_eq!(original.menu().status, "Slot 2 saved");
        let second = std::fs::read(slot_file(&slots, StateSlot::Two)).unwrap();
        original.shell.machine.key(b'X', 0x2d);
        original.menu_action(MenuAction::SaveStateSlot(StateSlot::One));
        assert_eq!(original.menu().status, "Slot 1 saved");
        assert_eq!(
            std::fs::read(slot_file(&slots, StateSlot::Two)).unwrap(),
            second
        );

        let mut restored = desktop(None);
        restored.menu_action(MenuAction::LoadStateSlot(StateSlot::One));
        assert_eq!(restored.menu().status, "Slot 1 loaded");
        assert!(restored.menu().running && restored.menu().paused);
        restored.menu_action(MenuAction::LoadStateSlot(StateSlot::Three));
        assert!(restored.menu().status.starts_with("Load slot 3 failed"));
        assert!(restored.menu().running && restored.menu().paused);
        restored.menu_action(MenuAction::SetPaused(false));
        restored.tick();
        assert_eq!(restored.shell.errorlevel(), b'X');
    }

    #[test]
    fn mounted_guest_uses_mouse_and_can_pause_and_resume() {
        let scratch = std::env::temp_dir().join(format!("dosa-pointer-{}", std::process::id()));
        std::fs::create_dir(&scratch).unwrap();
        let path = scratch.join("POINTER.COM");
        std::fs::write(
            &path,
            [
                0x31, 0xc0, 0xcd, 0x33, // Reset mouse.
                0xb8, 1, 0, 0xcd, 0x33, // Show cursor.
                0x31, 0xc0, 0xcd, 0x16, // Wait for a key.
                0xcd, 0x20, // Exit to the shell.
            ],
        )
        .unwrap();

        let mut desktop = Desktop::new(Shell::new(), None);
        desktop.menu_action(MenuAction::Mount {
            drive: 'C',
            source: path.clone(),
            kind: dosa_desktop::MountKind::File,
        });
        desktop.submit("POINTER");
        desktop.tick();
        assert!(desktop.menu().running);
        assert_eq!(desktop.pointer_mode(), PointerMode::Integrated);

        desktop.pointer(PointerEvent::Moved { x: 0, y: 0 });
        let cursor = desktop.shell.machine.mouse_cursor().unwrap();
        assert_eq!((cursor.x, cursor.y), (0, 0));
        desktop.menu_action(MenuAction::SetPaused(true));
        assert!(desktop.menu().paused);
        assert_eq!(desktop.pointer_mode(), PointerMode::Host);
        desktop.menu_action(MenuAction::SetCpu(CpuMode::I8086));
        assert_eq!(desktop.menu().cpu, CpuMode::I386);

        let enter = Input {
            key: Key::Enter,
            pressed: true,
            text: None,
            modifiers: Default::default(),
            scan_code: 0x1c,
        };
        desktop.input(enter.clone());
        desktop.tick();
        assert!(desktop.menu().running);
        desktop.menu_action(MenuAction::SetPaused(false));
        desktop.input(enter);
        desktop.tick();
        assert!(!desktop.menu().running);
        assert_eq!(desktop.pointer_mode(), PointerMode::Host);

        drop(desktop);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(scratch).unwrap();
    }

    #[test]
    fn shell_backspace_erases_across_wrap_and_preserves_the_prompt() {
        for (start_row, visible_row) in [(0, 0), (24, 23)] {
            let mut desktop = Desktop::new(Shell::new(), None);
            desktop.shell.machine.clear_console();
            desktop
                .shell
                .machine
                .write_console(&b"\r\n".repeat(start_row));
            desktop.write_prompt();

            let prompt = desktop.shell.prompt();
            let typed = "x".repeat(79 - prompt.len()) + "y";
            desktop.input(Input {
                key: Key::Character('x'),
                pressed: true,
                text: Some(typed.clone()),
                modifiers: Default::default(),
                scan_code: 0x2d,
            });
            let backspace = Input {
                key: Key::Backspace,
                pressed: true,
                text: None,
                modifiers: Default::default(),
                scan_code: 0x0e,
            };
            desktop.input(backspace.clone());

            let Video::Text(frame) = desktop.shell.machine.video() else {
                panic!("expected shell text mode");
            };
            let row_start = usize::from(visible_row) * usize::from(frame.columns) * 2;
            assert_eq!(frame.cursor, Some((79, visible_row)));
            assert_eq!(frame.cells[row_start + 78 * 2], b'x');
            assert_eq!(frame.cells[row_start + 79 * 2], b' ');

            // Erase the remaining input and press Backspace at the empty prompt.
            for _ in 0..typed.len() {
                desktop.input(backspace.clone());
            }
            assert!(desktop.line.is_empty());
            let Video::Text(frame) = desktop.shell.machine.video() else {
                panic!("expected shell text mode");
            };
            assert_eq!(frame.cursor, Some((prompt.len() as u16, visible_row)));
            for (offset, character) in prompt.bytes().enumerate() {
                assert_eq!(frame.cells[row_start + offset * 2], character);
            }
        }
    }
}
