use std::sync::LazyLock;
use std::time::Instant;

use dosa_desktop::{App, Frame, Input, Key};
use dosa_dos::{RunState, TextFrame, Video};
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
    exit: bool,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    started: Instant,
}

impl Desktop {
    pub fn new(shell: Shell, initial: Option<&str>) -> Self {
        let mut desktop = Self {
            shell,
            line: String::new(),
            running: false,
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

    fn write_prompt(&mut self) {
        self.shell
            .machine
            .write_console(self.shell.prompt().as_bytes());
    }

    fn submit(&mut self, line: &str) {
        self.shell.machine.write_console(b"\r\n");
        match self.shell.execute(line) {
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
                return;
            }
            Ok(Command::Launched) => {
                self.running = true;
                return;
            }
            Err(error) => self
                .shell
                .machine
                .write_console(format!("{error}\r\n").as_bytes()),
        }
        self.write_prompt();
    }

    fn render(&mut self) {
        match self.shell.machine.video() {
            Video::Text(frame) => {
                self.width = u32::from(frame.columns) * 8;
                self.height = u32::from(frame.rows) * 16;
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
                for (&index, pixel) in indices.iter().zip(self.pixels.chunks_exact_mut(4)) {
                    let [r, g, b] = palette[usize::from(index)];
                    pixel.copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
    }
}

impl App for Desktop {
    fn tick(&mut self) {
        if self.running {
            match self.shell.machine.run(50_000) {
                Ok(RunState::Idle | RunState::Exited(_)) => {
                    self.running = false;
                    self.shell.machine.write_console(b"\r\n");
                    self.write_prompt();
                }
                Ok(RunState::Running | RunState::WaitingForKey) => {}
                Err(error) => {
                    self.running = false;
                    self.shell.machine.stop();
                    self.shell
                        .machine
                        .write_console(format!("\r\n{error}\r\n").as_bytes());
                    self.write_prompt();
                }
            }
            // The guest's output has already been written to video memory.
            self.shell.machine.take_output();
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

    fn input(&mut self, event: Input) {
        if !event.pressed {
            if self.running {
                self.shell.machine.key_up(event.scan_code);
            }
            return;
        }
        if self.running {
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
    for (cell, data) in frame.cells.chunks_exact(2).enumerate() {
        let (column, row) = (cell % columns, cell / columns);
        let cursor = blink && frame.cursor == Some((column as u16, row as u16));
        let glyph = GLYPHS[usize::from(data[0])];
        let color = VGA[usize::from(data[1] & 0x0f)];
        let background = VGA[usize::from(data[1] >> 4 & 0x07)];
        let visible = data[1] & 0x80 == 0 || blink;
        for y in 0..16 {
            let start = ((row * 16 + y) * width + column * 8) * 4;
            for (x, pixel) in pixels[start..start + 32].chunks_exact_mut(4).enumerate() {
                let [r, g, b] = if cursor && y >= 14 || visible && glyph[y / 2] & (1 << x) != 0 {
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
