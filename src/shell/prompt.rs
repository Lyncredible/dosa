use std::{cell::OnceCell, error::Error, fmt::Write};

use dosa_dos::DOS_VERSION;

use super::{Command, Shell};

pub(super) const HELP: &str = "PROMPT [text]\r\n\
    PROMPT restores the default. SET PROMPT can also change the text.\r\n\
    $P drive and path   $N drive      $V DOS version\r\n\
    $D date            $T time       $_ newline\r\n\
    $G >  $L <  $B |   $Q =   $$ $   $E Escape    $H Backspace\r\n";

const DEFAULT_PROMPT: &str = "$P$G ";

impl Shell {
    pub(super) fn current_path(&self) -> String {
        let directory = self.directories.get(&self.drive).map_or("", String::as_str);
        format!("{}:\\{directory}", self.drive)
    }

    pub(super) fn set_prompt(&mut self, template: &str) -> Result<Command, Box<dyn Error>> {
        if template.trim_end() == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        self.machine.environment_mut().set("PROMPT", template)?;
        Ok(Command::Prompt)
    }

    pub fn prompt(&self) -> String {
        let template = self
            .machine
            .environment()
            .get("PROMPT")
            .unwrap_or(DEFAULT_PROMPT);
        let clock = OnceCell::new();
        let mut output = String::with_capacity(template.len());
        let mut chars = template.chars();
        while let Some(character) = chars.next() {
            if character != '$' {
                output.push(character);
                continue;
            }
            match chars.next().map(|code| code.to_ascii_uppercase()) {
                Some('P') => output.push_str(&self.current_path()),
                Some('N') => output.push(self.drive),
                Some('V') => {
                    let [major, minor] = DOS_VERSION;
                    let _ = write!(output, "MS-DOS Version {major}.{minor:02}");
                }
                Some('D') => {
                    let (year, month, day, weekday) =
                        clock.get_or_init(|| self.machine.clock_snapshot()).date();
                    let day_name =
                        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][usize::from(weekday)];
                    let _ = write!(output, "{day_name} {month:02}-{day:02}-{year:04}");
                }
                Some('T') => {
                    let (hour, minute, second, hundredths) =
                        clock.get_or_init(|| self.machine.clock_snapshot()).time();
                    let _ = write!(output, "{hour:02}:{minute:02}:{second:02}.{hundredths:02}");
                }
                Some('G') => output.push('>'),
                Some('L') => output.push('<'),
                Some('B') => output.push('|'),
                Some('Q') => output.push('='),
                Some('$') => output.push('$'),
                Some('_') => output.push_str("\r\n"),
                Some('E') => output.push('\x1b'),
                Some('H') => output.push('\x08'),
                _ => {}
            }
        }
        output
    }
}
