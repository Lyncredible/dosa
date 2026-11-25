#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Text,
    Escape,
    Parameters,
}

#[derive(Clone, Copy)]
pub(super) struct Ansi {
    state: State,
    values: [u16; 16],
    count: usize,
    invalid: bool,
    mode: bool,
    foreground: u8,
    background: u8,
    bright: bool,
    blink: bool,
    reverse: bool,
    conceal: bool,
    pub(super) wrap: bool,
    pub(super) saved_cursor: Option<(usize, usize)>,
}

pub(super) enum Byte {
    Text(u8),
    LiteralEscape(u8),
    Pending,
    Command(u8),
}

impl Default for Ansi {
    fn default() -> Self {
        Self {
            state: State::Text,
            values: [0; 16],
            count: 1,
            invalid: false,
            mode: false,
            foreground: 7,
            background: 0,
            bright: false,
            blink: false,
            reverse: false,
            conceal: false,
            wrap: true,
            saved_cursor: None,
        }
    }
}

impl Ansi {
    pub(super) fn consume(&mut self, byte: u8) -> Byte {
        if byte == 27 {
            let previous = std::mem::replace(&mut self.state, State::Escape);
            return match previous {
                State::Escape => Byte::Text(27),
                _ => Byte::Pending,
            };
        }
        match self.state {
            State::Text => Byte::Text(byte),
            State::Escape if byte == b'[' => {
                self.state = State::Parameters;
                self.values.fill(0);
                self.count = 1;
                self.mode = false;
                self.invalid = false;
                Byte::Pending
            }
            State::Escape => {
                self.state = State::Text;
                Byte::LiteralEscape(byte)
            }
            State::Parameters => {
                match byte {
                    b'0'..=b'9' => {
                        let value = &mut self.values[self.count - 1];
                        *value = value
                            .saturating_mul(10)
                            .saturating_add(u16::from(byte - b'0'));
                    }
                    b';' if self.count < self.values.len() => self.count += 1,
                    b'=' if !self.mode && self.count == 1 && self.values[0] == 0 => {
                        self.mode = true;
                    }
                    0x40..=0x7e => {
                        self.state = State::Text;
                        return if self.invalid {
                            Byte::Pending
                        } else {
                            Byte::Command(byte)
                        };
                    }
                    0..=0x1f => {
                        self.state = State::Text;
                        return Byte::Text(byte);
                    }
                    _ => self.invalid = true,
                }
                Byte::Pending
            }
        }
    }

    pub(super) fn parameter(&self, index: usize) -> u16 {
        self.values[index]
    }

    pub(super) fn mode_command(&self) -> bool {
        self.mode
    }

    pub(super) fn set_graphics_rendition(&mut self) {
        const VGA_COLORS: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
        for value in self.values[..self.count].iter().copied() {
            match value {
                0 => {
                    self.foreground = 7;
                    self.background = 0;
                    self.bright = false;
                    self.blink = false;
                    self.reverse = false;
                    self.conceal = false;
                }
                1 => self.bright = true,
                5 => self.blink = true,
                7 => self.reverse = true,
                8 => self.conceal = true,
                22 => self.bright = false,
                25 => self.blink = false,
                27 => self.reverse = false,
                28 => self.conceal = false,
                30..=37 => self.foreground = VGA_COLORS[usize::from(value - 30)],
                39 => self.foreground = 7,
                40..=47 => self.background = VGA_COLORS[usize::from(value - 40)],
                49 => self.background = 0,
                _ => {}
            }
        }
    }

    pub(super) fn attribute(&self) -> u8 {
        let (foreground, background) = if self.reverse {
            (self.background, self.foreground)
        } else {
            (self.foreground, self.background)
        };
        if self.conceal {
            return background << 4 | background;
        }
        (background << 4) | foreground | (u8::from(self.bright) << 3) | (u8::from(self.blink) << 7)
    }
}
