use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

use super::{Command, Shell, split_word};

const MAX_BATCH_BYTES: u64 = 1024 * 1024;
const MAX_BATCH_LINES: usize = 10_000;
const MAX_COMMAND_BYTES: usize = 8192;
const MAX_CALL_DEPTH: usize = 16;

pub struct BatchStep {
    /// The command line as it appears in the DOS console when echo is on.
    pub echo: Option<String>,
    pub command: Command,
}

struct Script {
    name: String,
    lines: Vec<String>,
    labels: BTreeMap<String, usize>,
}

impl Script {
    fn new(name: String, source: &str) -> Result<Self, String> {
        let lines: Vec<_> = source
            .trim_start_matches('\u{feff}')
            .lines()
            .take(MAX_BATCH_LINES + 1)
            .map(str::to_owned)
            .collect();
        if lines.len() > MAX_BATCH_LINES {
            return Err(format!(
                "{name}: batch file has more than {MAX_BATCH_LINES} lines"
            ));
        }
        let mut labels = BTreeMap::new();
        for (index, line) in lines.iter().enumerate() {
            if let Some(label) = line.trim().strip_prefix(':')
                && !label.is_empty()
                && !label.starts_with(':')
            {
                labels
                    .entry(label.to_ascii_uppercase())
                    .or_insert(index + 1);
            }
        }
        Ok(Self {
            name,
            lines,
            labels,
        })
    }

    fn label(&self, name: &str) -> Result<usize, String> {
        self.labels
            .get(&name.to_ascii_uppercase())
            .copied()
            .ok_or_else(|| format!("batch label {name:?} was not found"))
    }
}

struct Frame {
    script: Arc<Script>,
    next_line: usize,
    args: Vec<String>,
    tail: String,
}

impl Frame {
    fn new(
        script: Arc<Script>,
        next_line: usize,
        command: &str,
        args: &str,
    ) -> Result<Self, Box<dyn Error>> {
        if args.len() > MAX_COMMAND_BYTES {
            return Err(command_length_error().into());
        }
        let mut parameters = vec![command.to_owned()];
        let mut rest = args;
        for _ in 0..9 {
            if rest.is_empty() {
                break;
            }
            let (_, next) = split_word(rest)?;
            // Keep quotation marks for parameter substitution. The shell
            // interprets these when splitting a later command or path.
            let parameter = rest[..rest.len() - next.len()].trim_end();
            parameters.push(parameter.to_owned());
            rest = next;
        }
        Ok(Self {
            script,
            next_line,
            args: parameters,
            tail: args.to_owned(),
        })
    }

    fn expand(
        &self,
        mut rest: &str,
        errorlevel: u8,
        environment: &dosa_dos::Environment,
    ) -> Result<String, String> {
        if rest.len() > MAX_COMMAND_BYTES {
            return Err(command_length_error());
        }
        let mut output = String::with_capacity(rest.len());
        while let Some((literal, after_percent)) = rest.split_once('%') {
            append_command_text(&mut output, literal)?;
            let Some(&next) = after_percent.as_bytes().first() else {
                return Err("trailing % in batch line; use %% for a literal %".to_owned());
            };
            match next {
                b'%' => append_command_text(&mut output, "%")?,
                b'*' => append_command_text(&mut output, &self.tail)?,
                b'0'..=b'9' => {
                    if let Some(parameter) = self.args.get(usize::from(next - b'0')) {
                        append_command_text(&mut output, parameter)?;
                    }
                }
                _ => {
                    if let Some((name, after)) = after_percent.split_once('%') {
                        if name.eq_ignore_ascii_case("ERRORLEVEL") {
                            append_command_text(&mut output, &errorlevel.to_string())?;
                        } else if let Some(value) = environment.get(name) {
                            append_command_text(&mut output, value)?;
                        }
                        rest = after;
                        continue;
                    }
                    return Err("unterminated environment expansion in batch line".to_owned());
                }
            }
            rest = &after_percent[1..];
        }
        append_command_text(&mut output, rest)?;
        Ok(output)
    }
}

pub(super) struct Batch {
    frames: Vec<Frame>,
    remaining: usize,
    echo: bool,
    location: Option<(Arc<Script>, usize)>,
}

struct Line {
    command: String,
    echo: bool,
}

impl Batch {
    fn new(frame: Frame) -> Self {
        Self {
            frames: vec![frame],
            remaining: MAX_BATCH_LINES,
            echo: true,
            location: None,
        }
    }

    fn frame(&self) -> &Frame {
        self.frames
            .last()
            .expect("an executing line has a batch frame")
    }

    fn push(&mut self, frame: Frame) -> Result<(), String> {
        if self.frames.len() >= MAX_CALL_DEPTH {
            return Err(format!(
                "batch CALL depth limit of {MAX_CALL_DEPTH} exceeded"
            ));
        }
        self.frames.push(frame);
        Ok(())
    }

    fn next(
        &mut self,
        errorlevel: u8,
        environment: &dosa_dos::Environment,
    ) -> Result<Option<Line>, String> {
        while let Some(frame) = self.frames.last_mut() {
            let Some(raw) = frame.script.lines.get(frame.next_line) else {
                self.frames.pop();
                continue;
            };
            frame.next_line += 1;
            self.location = Some((Arc::clone(&frame.script), frame.next_line));
            if self.remaining == 0 {
                return Err(format!("batch line limit of {MAX_BATCH_LINES} exceeded"));
            }
            self.remaining -= 1;
            let raw = raw.trim_start();
            let (command, quiet) = raw
                .strip_prefix('@')
                .map_or((raw, false), |command| (command.trim_start(), true));
            let first = command.split_whitespace().next().unwrap_or("");
            if command.is_empty() || command.starts_with(':') || first.eq_ignore_ascii_case("REM") {
                return Ok(Some(Line {
                    command: String::new(),
                    echo: false,
                }));
            }
            return Ok(Some(Line {
                command: frame.expand(command, errorlevel, environment)?,
                echo: self.echo && !quiet,
            }));
        }
        Ok(None)
    }

    fn contextualize(self, error: Box<dyn Error>) -> Box<dyn Error> {
        match self.location {
            Some((file, line)) => format!("{}:{line}: {error}", file.name).into(),
            None => error,
        }
    }
}

impl Shell {
    pub fn batch_active(&self) -> bool {
        self.batch.is_some()
    }

    pub fn errorlevel(&self) -> u8 {
        self.errorlevel
    }

    pub fn guest_finished(&mut self, code: u8) {
        self.errorlevel = code;
    }

    /// Stop an active batch and attach its current source line to an error.
    pub fn guest_failed(&mut self, error: Box<dyn Error>) -> Box<dyn Error> {
        self.errorlevel = 1;
        match self.batch.take() {
            Some(batch) => batch.contextualize(error),
            None => error,
        }
    }

    /// Execute one batch line when no guest program is running.
    pub fn step_batch(&mut self) -> Result<Option<BatchStep>, Box<dyn Error>> {
        let result = self.step_batch_line();
        result.map_err(|error| self.guest_failed(error))
    }

    fn step_batch_line(&mut self) -> Result<Option<BatchStep>, Box<dyn Error>> {
        let Some(batch) = self.batch.as_mut() else {
            return Ok(None);
        };
        let Some(line) = batch.next(self.errorlevel, self.machine.environment())? else {
            self.batch = None;
            return Ok(None);
        };
        let echo = line
            .echo
            .then(|| format!("{}{}\r\n", self.prompt(), line.command));
        let command = self.execute_batch_command(&line.command)?;
        Ok(Some(BatchStep { echo, command }))
    }

    fn execute_batch_command(&mut self, mut line: &str) -> Result<Command, Box<dyn Error>> {
        // Reject shell operators that the batch interpreter cannot execute.
        let mut quoted = false;
        for character in line.chars() {
            match character {
                '"' => quoted = !quoted,
                '|' | '<' | '>' | '&' if !quoted => {
                    return Err(
                        "batch pipes, redirection, and command chaining are unsupported".into(),
                    );
                }
                _ => {}
            }
        }
        loop {
            let (word, args) = split_word(line.trim())?;
            match word.to_ascii_uppercase().as_str() {
                "IF" => {
                    let (negate, threshold, command) = parse_condition(args)?;
                    if (self.errorlevel >= threshold) == negate {
                        return Ok(Command::Prompt);
                    }
                    line = command;
                }
                "CALL" => return self.call_batch(args),
                "GOTO" => return self.goto_batch(args),
                "GOTO:EOF" if args.is_empty() => return self.goto_batch(":EOF"),
                "EXIT" if !args.is_empty() => return self.exit_batch(args),
                "EXIT" => {
                    self.batch = None;
                    return Ok(Command::Exit);
                }
                "ECHO." if args.is_empty() => return Ok(Command::Print(b"\r\n".to_vec())),
                "ECHO" => return Ok(self.echo_batch(args)),
                "MOUNT" | "IMGMOUNT" => return Err("mount host paths using the interactive shell or --mount before starting a batch file".into()),
                "BOOT" => return Err("boot host disk images from the interactive shell".into()),
                "FOR" | "SETLOCAL" | "ENDLOCAL" | "SHIFT" | "PAUSE" | "CHOICE" => {
                    return Err(format!("batch command {word} is unsupported").into());
                }
                _ => return self.execute(line),
            }
        }
    }

    fn echo_batch(&mut self, args: &str) -> Command {
        let batch = self.batch.as_mut().expect("executing a batch command");
        if args.eq_ignore_ascii_case("OFF") || args.eq_ignore_ascii_case("ON") {
            batch.echo = args.eq_ignore_ascii_case("ON");
            Command::Prompt
        } else if args.is_empty() {
            let state = if batch.echo { "on" } else { "off" };
            Command::Print(format!("ECHO is {state}.\r\n").into_bytes())
        } else {
            Command::Print(format!("{args}\r\n").into_bytes())
        }
    }

    fn goto_batch(&mut self, label: &str) -> Result<Command, Box<dyn Error>> {
        let batch = self.batch.as_mut().expect("executing a batch command");
        if label.eq_ignore_ascii_case(":EOF") {
            batch.frames.pop();
        } else {
            let label = label.strip_prefix(':').unwrap_or(label);
            if label.is_empty() {
                return Err("usage: GOTO label".into());
            }
            let frame = batch.frames.last_mut().expect("executing a batch command");
            frame.next_line = frame.script.label(label)?;
        }
        Ok(Command::Prompt)
    }

    fn exit_batch(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        let (switch, status) = split_word(args)?;
        if !switch.eq_ignore_ascii_case("/B") {
            return Err("usage: EXIT /B [status]".into());
        }
        if !status.is_empty() {
            self.errorlevel = status.parse().map_err(|_| "EXIT /B status must be 0–255")?;
        }
        self.batch
            .as_mut()
            .expect("executing a batch command")
            .frames
            .pop();
        Ok(Command::Prompt)
    }

    fn call_batch(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        let (word, tail) = split_word(args)?;
        if word.is_empty() {
            return Err("usage: CALL filename.BAT [arguments] or CALL :label [arguments]".into());
        }
        if let Some(label) = word.strip_prefix(':') {
            let batch = self.batch.as_mut().expect("executing a batch command");
            let script = Arc::clone(&batch.frame().script);
            let target = script.label(label)?;
            batch.push(Frame::new(script, target, word, tail)?)?;
        } else {
            let (drive, mut path) = self.resolve(word)?;
            if path
                .rsplit('\\')
                .next()
                .is_some_and(|name| !name.contains('.'))
            {
                path.push_str(".BAT");
            }
            if !is_batch(&path) {
                return Err("CALL requires a .BAT file or a batch label".into());
            }
            self.launch_batch(drive, &path, word, tail, true)?;
        }
        Ok(Command::Prompt)
    }

    pub(super) fn launch_batch(
        &mut self,
        drive: char,
        path: &str,
        word: &str,
        args: &str,
        call: bool,
    ) -> Result<(), Box<dyn Error>> {
        let mount = self.drive_mount(drive)?;
        let name = format!("{drive}:\\{path}");
        if mount.metadata(path)?.size > MAX_BATCH_BYTES {
            return Err(format!("{name}: batch file exceeds the 1 MiB size limit").into());
        }
        let bytes = mount.read(path)?;
        if bytes.len() as u64 > MAX_BATCH_BYTES {
            return Err(format!("{name}: batch file exceeds the 1 MiB size limit").into());
        }
        let source = std::str::from_utf8(
            &bytes[..bytes
                .iter()
                .position(|&byte| byte == 0x1a)
                .unwrap_or(bytes.len())],
        )
        .map_err(|error| format!("{name}: batch text must be ASCII or UTF-8 ({error})"))?;
        let frame = Frame::new(Arc::new(Script::new(name, source)?), 0, word, args)?;
        match self.batch.as_mut() {
            Some(batch) if call => batch.push(frame)?,
            Some(batch) => {
                // A BAT invoked without CALL replaces the current frame.
                *batch.frames.last_mut().expect("executing a batch command") = frame;
            }
            None => self.batch = Some(Batch::new(frame)),
        }
        Ok(())
    }
}

fn command_length_error() -> String {
    format!("batch command exceeds the {MAX_COMMAND_BYTES}-byte limit")
}

fn append_command_text(output: &mut String, value: &str) -> Result<(), String> {
    if value.len() > MAX_COMMAND_BYTES - output.len() {
        return Err(command_length_error());
    }
    output.push_str(value);
    Ok(())
}

fn parse_condition(args: &str) -> Result<(bool, u8, &str), Box<dyn Error>> {
    let (word, remaining) = split_word(args)?;
    let negate = word.eq_ignore_ascii_case("NOT");
    let (predicate, rest) = if negate {
        split_word(remaining)?
    } else {
        (word, remaining)
    };
    let (threshold, command) = split_word(rest)?;
    let usage = "usage: IF [NOT] ERRORLEVEL number command (number must be 0–255)";
    if !predicate.eq_ignore_ascii_case("ERRORLEVEL") || command.is_empty() {
        return Err(usage.into());
    }
    Ok((negate, threshold.parse().map_err(|_| usage)?, command))
}

pub(super) fn is_batch(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("BAT"))
}
