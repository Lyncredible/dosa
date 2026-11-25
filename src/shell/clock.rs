use dosa_dos::Machine;

use super::{Command, PendingInput, Shell};

pub(super) const DATE_HELP: &str =
    "DATE [mm-dd-yy]\r\n    Show or set the DOS session date. DATE /T displays it.\r\n";
pub(super) const TIME_HELP: &str = "TIME [hours:[minutes[:seconds[.hundredths]]][A|P]]\r\n    Show or set the DOS session time. TIME /T displays it.\r\n";

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

#[derive(Clone, Copy)]
pub(super) enum Component {
    Date,
    Time,
}

pub(super) struct ClockInput {
    component: Component,
    line: String,
}

impl Shell {
    pub(super) fn clock_command(&mut self, component: Component, args: &str) -> Command {
        if args == "/?" {
            let help = match component {
                Component::Date => DATE_HELP,
                Component::Time => TIME_HELP,
            };
            return Command::Print(help.as_bytes().to_vec());
        }
        if args.eq_ignore_ascii_case("/T") {
            return Command::Print(component.current(&self.machine).into_bytes());
        }
        if !args.is_empty() && component.apply(&mut self.machine, args) {
            return Command::Prompt;
        }
        let mut message = if args.is_empty() {
            component.current(&self.machine)
        } else {
            component.invalid().to_owned()
        };
        message.push_str(component.prompt());
        self.pending_input = Some(PendingInput::Clock(ClockInput {
            component,
            line: String::new(),
        }));
        Command::WaitForInput(message.into_bytes())
    }
}

impl ClockInput {
    pub(super) fn key(mut self, shell: &mut Shell, key: u8) -> Option<Command> {
        let response = match key {
            3 | 27 => return Some(Command::Print(b"\r\n".to_vec())),
            b'\r' | b'\n' => {
                let line = self.line.trim();
                if line.is_empty() || self.component.apply(&mut shell.machine, line) {
                    return Some(Command::Print(b"\r\n".to_vec()));
                }
                self.line.clear();
                format!(
                    "\r\n{}{}",
                    self.component.invalid(),
                    self.component.prompt()
                )
                .into_bytes()
            }
            8 if self.line.pop().is_some() => b"\x08 \x08".to_vec(),
            b' '..=b'~' if self.line.len() < 24 => {
                self.line.push(char::from(key));
                vec![key]
            }
            _ => {
                shell.pending_input = Some(PendingInput::Clock(self));
                return None;
            }
        };
        shell.pending_input = Some(PendingInput::Clock(self));
        Some(Command::WaitForInput(response))
    }
}

impl Component {
    fn current(self, machine: &Machine) -> String {
        let clock = machine.clock_snapshot();
        match self {
            Self::Date => {
                let (year, month, day, weekday) = clock.date();
                let weekday = DAYS[usize::from(weekday)];
                format!("Current date is {weekday} {month:02}-{day:02}-{year:04}\r\n")
            }
            Self::Time => {
                let (hour, minute, second, hundredths) = clock.time();
                format!("Current time is {hour:02}:{minute:02}:{second:02}.{hundredths:02}\r\n")
            }
        }
    }

    fn apply(self, machine: &mut Machine, text: &str) -> bool {
        match self {
            Self::Date => apply_date(machine, text),
            Self::Time => apply_time(machine, text).unwrap_or(false),
        }
    }

    fn invalid(self) -> &'static str {
        match self {
            Self::Date => "Invalid date\r\n",
            Self::Time => "Invalid time\r\n",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Date => "Enter new date (mm-dd-yy): ",
            Self::Time => "Enter new time: ",
        }
    }
}

fn apply_date(machine: &mut Machine, text: &str) -> bool {
    let mut fields = text.split(['.', '-', '/']);
    let (Some(month), Some(day), Some(year), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let (Ok(month), Ok(day), Ok(number)) = (month.parse(), day.parse(), year.parse::<u16>()) else {
        return false;
    };
    let year = match (year.len(), number) {
        (2, 80..=99) => 1900 + number,
        (4, _) => number,
        _ => return false,
    };
    machine.set_date(year, month, day)
}

fn apply_time(machine: &mut Machine, text: &str) -> Option<bool> {
    let (text, afternoon) = match text.as_bytes().last()? {
        b'A' | b'a' => (&text[..text.len() - 1], Some(false)),
        b'P' | b'p' => (&text[..text.len() - 1], Some(true)),
        _ => (text, None),
    };
    let (hour, rest) = text.trim_end().split_once(':')?;
    let mut hour = hour.parse::<u8>().ok()?;
    if let Some(afternoon) = afternoon {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if afternoon { 12 } else { 0 };
    }
    let (minute, rest) = rest.split_once(':').unwrap_or((rest, ""));
    let (second, hundredths) = rest.split_once('.').unwrap_or((rest, ""));
    let number = |text: &str| {
        if text.is_empty() {
            Some(0u8)
        } else {
            text.parse::<u8>().ok()
        }
    };
    Some(machine.set_time(hour, number(minute)?, number(second)?, number(hundredths)?))
}
