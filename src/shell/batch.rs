use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::sync::Arc;

use dosa_dos::ShellIo;

use super::for_loop::ForLoop;
use super::{Command, Shell, split_word};

const MAX_BATCH_BYTES: u64 = 1024 * 1024;
const MAX_BATCH_LINES: usize = 10_000;
const MAX_COMMAND_BYTES: usize = 8192;
const MAX_CALL_DEPTH: usize = 16;
pub(super) const SHIFT_HELP: &str =
    "SHIFT\r\n    Move each batch argument down one position, starting at %0.\r\n";
pub(super) const PAUSE_HELP: &str = "PAUSE\r\n    Wait for a key before continuing the batch.\r\n";
pub const PAUSE_MESSAGE: &[u8] = b"Press any key to continue . . .";

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
    args: VecDeque<String>,
    tail: String,
    for_loop: Option<(ForLoop, bool)>,
    io: Option<ShellIo>,
}

impl Frame {
    fn new(
        script: Arc<Script>,
        next_line: usize,
        command: &str,
        args: &str,
        io: Option<ShellIo>,
    ) -> Result<Self, Box<dyn Error>> {
        if args.len() > MAX_COMMAND_BYTES {
            return Err(command_length_error().into());
        }
        let mut parameters = VecDeque::from([command.to_owned()]);
        let mut rest = args;
        while !rest.is_empty() {
            let (_, next) = split_word(rest)?;
            // Keep quotation marks for parameter substitution. The shell
            // interprets these when splitting a later command or path.
            let parameter = rest[..rest.len() - next.len()].trim_end();
            parameters.push_back(parameter.to_owned());
            rest = next;
        }
        Ok(Self {
            script,
            next_line,
            args: parameters,
            tail: args.to_owned(),
            for_loop: None,
            io,
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
    prompt_loop: bool,
    location: Option<(Arc<Script>, usize)>,
    pause: Option<Pause>,
}

#[derive(Clone, Copy)]
enum Pause {
    Key,
    ConfirmAbort,
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
            prompt_loop: false,
            location: None,
            pause: None,
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
            if let Some((for_loop, echo)) = &mut frame.for_loop {
                self.location =
                    (frame.next_line != 0).then(|| (Arc::clone(&frame.script), frame.next_line));
                if let Some(command) = for_loop.next_command(MAX_COMMAND_BYTES)? {
                    if self.remaining == 0 {
                        return Err(format!("batch line limit of {MAX_BATCH_LINES} exceeded"));
                    }
                    self.remaining -= 1;
                    return Ok(Some(Line {
                        command,
                        echo: self.echo && *echo,
                    }));
                }
                frame.for_loop = None;
            }
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
    pub(super) fn start_prompt_loop(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        let for_loop = ForLoop::parse(self, args, MAX_BATCH_LINES)?;
        let script = Arc::new(Script::new("FOR".to_owned(), "")?);
        let mut frame = Frame::new(script, 0, "FOR", "", None)?;
        frame.for_loop = Some((for_loop, false));
        let mut batch = Batch::new(frame);
        batch.prompt_loop = true;
        self.batch = Some(batch);
        Ok(Command::Prompt)
    }

    pub(super) fn command_from_batch_file(&self) -> bool {
        self.batch
            .as_ref()
            .is_some_and(|batch| !batch.prompt_loop || batch.frames.len() > 1)
    }

    pub(super) fn batch_io(&self) -> Option<&ShellIo> {
        self.batch.as_ref()?.frames.last()?.io.as_ref()
    }

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
        self.pending_input = None;
        self.pending_redirects = None;
        self.errorlevel = 1;
        match self.batch.take() {
            Some(batch) => batch.contextualize(error),
            None => error,
        }
    }

    /// Execute one batch line when no guest program is running or waiting for input.
    pub fn step_batch(&mut self) -> Result<Option<BatchStep>, Box<dyn Error>> {
        if self.batch_waiting() || self.input_waiting() {
            return Ok(None);
        }
        let result = self.step_batch_line();
        result.map_err(|error| self.guest_failed(error))
    }

    pub fn batch_waiting(&self) -> bool {
        self.batch
            .as_ref()
            .is_some_and(|batch| batch.pause.is_some())
    }

    /// Handle a DOS key during PAUSE and return the console response.
    pub fn resume_batch(&mut self, key: u8) -> Option<&'static [u8]> {
        let batch = self.batch.as_mut()?;
        match (batch.pause?, key.to_ascii_uppercase()) {
            (Pause::Key, 3) => {
                batch.pause = Some(Pause::ConfirmAbort);
                Some(b"\r\nTerminate batch job (Y/N)? ")
            }
            (Pause::Key, _) => {
                batch.pause = None;
                Some(b"\r\n")
            }
            (Pause::ConfirmAbort, b'Y') => {
                self.batch = None;
                self.errorlevel = 3;
                Some(b"Y\r\n")
            }
            (Pause::ConfirmAbort, b'N') => {
                batch.pause = None;
                Some(b"N\r\n")
            }
            (Pause::ConfirmAbort, _) => None,
        }
    }

    fn step_batch_line(&mut self) -> Result<Option<BatchStep>, Box<dyn Error>> {
        let Some(batch) = self.batch.as_mut() else {
            return Ok(None);
        };
        let Some(line) = batch.next(self.errorlevel, self.machine.environment())? else {
            self.batch = None;
            return Ok(None);
        };
        let mut echo = line
            .echo
            .then(|| format!("{}{}\r\n", self.prompt(), line.command));
        let mut io = self
            .batch_io()
            .map(|parent| {
                self.machine.prepare_shell_io(
                    self.drive,
                    &self.directories,
                    Some(parent),
                    None,
                    None,
                )
            })
            .transpose()?;
        if let (Some(text), Some(streams)) = (&echo, &mut io) {
            let bytes = self.machine.code_page().encode_text(text);
            if self.machine.write_shell_stdout(streams, &bytes)? {
                echo = None;
            }
        }
        let mut command = self.execute_batch_command(&line.command, line.echo, &mut io)?;
        if let Some(streams) = io {
            command = self.apply_redirection(streams, command)?;
        }
        Ok(Some(BatchStep { echo, command }))
    }

    fn execute_batch_command(
        &mut self,
        mut line: &str,
        echo: bool,
        io: &mut Option<ShellIo>,
    ) -> Result<Command, Box<dyn Error>> {
        // Reject shell operators that the batch interpreter cannot execute.
        let mut quoted = false;
        for character in line.chars() {
            match character {
                '"' => quoted = !quoted,
                '|' | '&' if !quoted => {
                    return Err("batch pipes and command chaining are unsupported".into());
                }
                _ => {}
            }
        }
        loop {
            let (word, args) = split_word(line.trim())?;
            if !word.eq_ignore_ascii_case("IF")
                && !word.eq_ignore_ascii_case("FOR")
                && let Some(request) = super::redirection::parse(line)?
            {
                io.take(); // This line prepares its own overlay and routes its output.
                return self.execute_redirected(request, true);
            }
            match word.to_ascii_uppercase().as_str() {
                "IF" | "PAUSE" | "FOR" if args == "/?" => return self.execute_command(line, io),
                "IF" => {
                    let (matched, command) = self.test_condition(args)?;
                    if !matched {
                        return Ok(Command::Prompt);
                    }
                    line = command;
                }
                "CALL" => return self.call_batch(args, io.take()),
                "GOTO" => return self.goto_batch(args),
                "GOTO:EOF" if args.is_empty() => return self.goto_batch(":EOF"),
                "PAUSE" => return self.pause_batch(args),
                "SHIFT" if args == "/?" => return self.execute_command(line, io),
                "SHIFT" => {
                    if !args.is_empty() {
                        return Err("usage: SHIFT".into());
                    }
                    let batch = self.batch.as_mut().expect("executing a batch command");
                    let frame = batch.frames.last_mut().expect("an executing line has a batch frame");
                    frame.args.pop_front();
                    return Ok(Command::Prompt);
                }
                "EXIT" if !args.is_empty() => return self.exit_batch(args),
                "EXIT" => {
                    self.batch = None;
                    return Ok(Command::Exit);
                }
                "ECHO." if args.is_empty() => return Ok(Command::Print(b"\r\n".to_vec())),
                "ECHO" => return Ok(self.echo_batch(args)),
                "MOUNT" | "IMGMOUNT" => return Err("mount host paths using the interactive shell or --mount before starting a batch file".into()),
                "BOOT" => return Err("boot host disk images from the interactive shell".into()),
                "FOR" => {
                    let batch = self.batch.as_ref().expect("executing a batch command");
                    if batch.frame().for_loop.is_some() {
                        return Err("nested FOR commands require a separate batch frame".into());
                    }
                    let for_loop = ForLoop::parse(self, args, batch.remaining)?;
                    let frame = self.batch.as_mut().unwrap().frames.last_mut().unwrap();
                    frame.for_loop = Some((for_loop, echo));
                    return Ok(Command::Prompt);
                }
                "SETLOCAL" | "ENDLOCAL" | "CHOICE" => {
                    return Err(format!("batch command {word} is unsupported").into());
                }
                _ => return self.execute_command(line, io),
            }
        }
    }

    pub(super) fn pause_batch(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        if !args.is_empty() {
            return Err("usage: PAUSE".into());
        }
        self.batch
            .as_mut()
            .expect("executing a batch command")
            .pause = Some(Pause::Key);
        Ok(Command::Pause)
    }

    pub(super) fn echo_batch(&mut self, args: &str) -> Command {
        let batch = self.batch.as_mut().expect("executing a batch command");
        if args.eq_ignore_ascii_case("OFF") || args.eq_ignore_ascii_case("ON") {
            batch.echo = args.eq_ignore_ascii_case("ON");
            Command::Prompt
        } else if args.is_empty() {
            let state = if batch.echo { "on" } else { "off" };
            Command::Print(format!("ECHO is {state}.\r\n").into_bytes())
        } else {
            Command::Print(self.machine.code_page().encode_text(&format!("{args}\r\n")))
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
            frame.for_loop = None;
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

    pub(super) fn call_batch(
        &mut self,
        args: &str,
        io: Option<ShellIo>,
    ) -> Result<Command, Box<dyn Error>> {
        let (word, tail) = split_word(args)?;
        if word.is_empty() {
            return Err("usage: CALL filename.BAT [arguments] or CALL :label [arguments]".into());
        }
        if let Some(label) = word.strip_prefix(':') {
            let batch = self.batch.as_mut().expect("executing a batch command");
            let script = Arc::clone(&batch.frame().script);
            let target = script.label(label)?;
            batch.push(Frame::new(script, target, word, tail, io)?)?;
        } else {
            let name = word.rsplit(['\\', '/', ':']).next().unwrap_or("");
            if name.contains('.') && !is_batch(name) {
                return Err("CALL requires a .BAT file or a batch label".into());
            }
            let (drive, path) = self.find_program(word, &["BAT"])?;
            self.launch_batch(drive, &path, word, tail, true, io)?;
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
        io: Option<ShellIo>,
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
        let bytes = bytes.split(|&byte| byte == 0x1a).next().unwrap_or_default();
        let source = match std::str::from_utf8(bytes) {
            Ok(text) => std::borrow::Cow::Borrowed(text),
            Err(_) => std::borrow::Cow::Owned(self.machine.code_page().decode_text(bytes)),
        };
        let frame = Frame::new(Arc::new(Script::new(name, &source)?), 0, word, args, io)?;
        match self.batch.as_mut() {
            Some(batch) if call || batch.prompt_loop && batch.frames.len() == 1 => {
                batch.push(frame)?
            }
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

pub(super) fn is_batch(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("BAT"))
}
