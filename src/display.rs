use std::sync::LazyLock;
use std::time::Instant;

use dosa_desktop::{
    App, CpuMode, Frame, Input, Key, MenuAction, MenuState, PointerEvent, PointerMode, Speed,
    TextMode,
};
use dosa_dos::{CpuProfile, MouseCursor, RunState, TextFrame, Video};
use dosa_mount::Mount;
use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, LATIN_FONTS, MISC_FONTS, UnicodeFonts,
};

use crate::shell::{Command, Shell};

const CP437_LOW: &str = "\0☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼";
const CP437_HIGH: &str = concat!(
    "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒ",
    "áíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐",
    "└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀",
    "αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ "
);
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

static GLYPHS: LazyLock<[[u8; 8]; 256]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let character = match index {
            0..32 => CP437_LOW.chars().nth(index).unwrap(),
            32..127 => index as u8 as char,
            127 => '⌂',
            _ => CP437_HIGH.chars().nth(index - 128).unwrap(),
        };
        BASIC_FONTS
            .get(character)
            .or_else(|| LATIN_FONTS.get(character))
            .or_else(|| BOX_FONTS.get(character))
            .or_else(|| BLOCK_FONTS.get(character))
            .or_else(|| GREEK_FONTS.get(character))
            .or_else(|| MISC_FONTS.get(character))
            .unwrap_or_else(|| BASIC_FONTS.get('?').unwrap())
    })
});

pub struct Desktop {
    shell: Shell,
    line: String,
    running: bool,
    paused: bool,
    settings: Settings,
    status: String,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cpu: CpuMode::I386,
            speed: Speed::Normal,
            text_mode: TextMode::Rows25,
        }
    }
}

impl Settings {
    pub fn apply(self, shell: &mut Shell) -> Result<(), dosa_dos::DosError> {
        shell.machine.set_cpu_profile(cpu_profile(self.cpu))?;
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
            settings,
            status: String::new(),
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
        self.shell
            .machine
            .write_console(self.shell.prompt().as_bytes());
    }

    fn submit(&mut self, line: &str) {
        self.shell.machine.write_console(b"\r\n");
        let result = self.shell.execute(line);
        self.apply_command(result);
        if !self.running && !self.exit && !self.shell.batch_active() {
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
                        self.shell.machine.write_console(echo.as_bytes());
                    }
                    self.apply_command(Ok(step.command));
                }
                Ok(None) => {
                    self.write_prompt();
                    break;
                }
                Err(error) => {
                    self.apply_command(Err(error));
                    self.write_prompt();
                    break;
                }
            }
            if self.running || self.exit {
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
            self.shell.machine.write_console(&[encode_cp437(character)]);
        }
    }
}

impl App for Desktop {
    fn audio(&mut self) -> Vec<f32> {
        let samples = self.shell.machine.take_audio();
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
            match self
                .shell
                .machine
                .run(instructions_per_tick(self.settings.speed))
            {
                Ok(state @ (RunState::Idle | RunState::Exited(_))) => {
                    if let RunState::Exited(code) = state {
                        self.shell.guest_finished(code);
                    }
                    self.running = false;
                    self.shell.machine.write_console(b"\r\n");
                    if !self.shell.batch_active() {
                        self.write_prompt();
                    }
                }
                Ok(RunState::Running | RunState::WaitingForKey | RunState::WaitingForInterrupt) => {
                }
                Err(error) => {
                    self.running = false;
                    self.shell.machine.stop();
                    let error = self.shell.guest_failed(error.into());
                    self.shell
                        .machine
                        .write_console(format!("\r\n{error}\r\n").as_bytes());
                    self.write_prompt();
                }
            }
            // The guest's output has already been written to video memory.
            self.shell.machine.take_output();
        }
        if !self.running && !self.paused && !self.exit && self.shell.batch_active() {
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
        MenuState {
            enabled: true,
            active_drive: self.shell.current_drive(),
            cpu: self.settings.cpu,
            speed: self.settings.speed,
            text_mode: self.settings.text_mode,
            running: self.running,
            paused: self.paused,
            status: self.status.clone(),
        }
    }

    fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::SaveScreenshot(path) => {
                self.render();
                self.status = match save_screenshot(&path, self.frame()) {
                    Ok(()) => "Screenshot saved".into(),
                    Err(error) => format!("Screenshot failed: {error}"),
                };
            }
            MenuAction::Mount {
                drive,
                source,
                kind,
            } if !self.running => {
                let result = match kind {
                    dosa_desktop::MountKind::SessionZip => Mount::open_session_zip(source),
                    _ => Mount::open(source),
                };
                self.status = match result
                    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
                    .and_then(|mount| self.shell.mount(drive, mount))
                {
                    Ok(()) => format!("Mounted {drive}:"),
                    Err(error) => format!("Mount failed: {error}"),
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
            MenuAction::SetPaused(paused) if self.running => {
                self.paused = paused;
                self.shell.machine.set_paused(paused);
                if paused {
                    self.shell.machine.mouse_buttons(0);
                }
            }
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

    fn input(&mut self, event: Input) {
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
                    .map_or(0, encode_cp437),
            };
            self.shell.machine.key(ascii, event.scan_code);
            return;
        }
        if self.shell.batch_active() {
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
                        self.shell.machine.write_console(&[encode_cp437(ch)]);
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

fn encode_cp437(character: char) -> u8 {
    if character.is_ascii() {
        return character as u8;
    }
    CP437_HIGH
        .chars()
        .position(|ch| ch == character)
        .map_or(b'?', |index| index as u8 + 128)
}

fn render_text(frame: TextFrame<'_>, width: usize, pixels: &mut [u8], blink: bool) {
    let columns = usize::from(frame.columns);
    let height = usize::from(frame.character_height);
    for (cell, data) in frame.cells.as_chunks::<2>().0.iter().enumerate() {
        let (column, row) = (cell % columns, cell / columns);
        let cursor = blink && frame.cursor == Some((column as u16, row as u16));
        let glyph = GLYPHS[usize::from(data[0])];
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
