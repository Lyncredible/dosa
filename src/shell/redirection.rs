use std::error::Error;

use dosa_dos::{ShellInput, ShellIo};

use super::{Command, PAUSE_MESSAGE, Shell, pending_input::PendingInput, split_word};

pub(super) struct Redirection<'a> {
    line: String,
    input: Option<&'a str>,
    output: Option<(&'a str, bool)>,
}

pub(super) fn parse(line: &str) -> Result<Option<Redirection<'_>>, &'static str> {
    let mut command = String::new();
    let mut input = None;
    let mut output = None;
    let mut remainder = line;
    let mut quoted = false;
    let mut compound = false;
    let mut end = 0;

    while end < remainder.len() {
        let character = remainder[end..].chars().next().unwrap();
        if character == '"' {
            quoted = !quoted;
        } else if !quoted && matches!(character, '<' | '>') {
            command.push_str(&remainder[..end]);
            command.push(' ');
            let after = &remainder[end + 1..];
            let (operand, append) = if character == '>' {
                after
                    .strip_prefix('>')
                    .map_or((after, false), |rest| (rest, true))
            } else {
                (after, false)
            };
            let (path, tail) = split_operand(operand.trim_start())?;
            if character == '<' {
                if input.replace(path).is_some() {
                    return Err("multiple standard input redirections");
                }
            } else if output.replace((path, append)).is_some() {
                return Err("multiple standard output redirections");
            }
            remainder = tail;
            end = 0;
            continue;
        }
        compound |= !quoted && matches!(character, '|' | '&');
        end += character.len_utf8();
    }
    if input.is_none() && output.is_none() {
        return Ok(None);
    }
    if compound {
        return Err("pipes and command chaining are unsupported");
    }
    command.push_str(remainder);
    if command.trim().is_empty() {
        return Err("redirection requires a DOS command");
    }
    Ok(Some(Redirection {
        line: command,
        input,
        output,
    }))
}

fn split_operand(rest: &str) -> Result<(&str, &str), &'static str> {
    let (path, tail) = if let Some(quoted) = rest.strip_prefix('"') {
        quoted
            .split_once('"')
            .ok_or("unterminated redirection path")?
    } else {
        let end = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '|' | '&'))
            .unwrap_or(rest.len());
        rest.split_at(end)
    };
    if path.is_empty() {
        return Err("redirection requires a file or device");
    }
    Ok((path, tail))
}

impl Shell {
    pub(super) fn execute_redirected(
        &mut self,
        request: Redirection<'_>,
        batch: bool,
    ) -> Result<Command, Box<dyn Error>> {
        let line = request.line.trim();
        let (word, args) = split_word(line)?;
        let word = word.to_ascii_uppercase();
        if matches!(word.as_str(), "BOOT" | "MOUNT" | "IMGMOUNT")
            || batch && matches!(word.as_str(), "GOTO" | "GOTO:EOF" | "SHIFT" | "EXIT")
        {
            return Err("redirection is unavailable for this command".into());
        }
        let mut io = Some(self.machine.prepare_shell_io(
            self.drive,
            &self.directories,
            self.batch_io(),
            request.input,
            request.output,
        )?);
        let command = match word.as_str() {
            "ECHO" if batch => self.echo_batch(args.trim_end()),
            "CALL" if batch => self.call_batch(args.trim_end(), io.take())?,
            "PAUSE" if batch && args.trim_end() != "/?" => self.pause_batch(args.trim_end())?,
            "ECHO." if batch && args.is_empty() => Command::Print(b"\r\n".to_vec()),
            _ => self.execute_command(line, &mut io)?,
        };
        match io {
            Some(io) => self.apply_redirection(io, command),
            None => Ok(command),
        }
    }

    pub(super) fn apply_redirection(
        &mut self,
        io: ShellIo,
        command: Command,
    ) -> Result<Command, Box<dyn Error>> {
        let result = self.route_redirected_command(io, command);
        if result.is_err() {
            self.pending_input = None;
        }
        result
    }

    fn route_redirected_command(
        &mut self,
        mut io: ShellIo,
        mut command: Command,
    ) -> Result<Command, Box<dyn Error>> {
        let mut display = Vec::new();
        let mut skip_lf = false;
        loop {
            match command {
                Command::Pause => {
                    self.pending_input = Some(PendingInput::BatchPause);
                    command = Command::WaitForInput(PAUSE_MESSAGE.to_vec());
                }
                Command::Print(bytes) => {
                    if !self.machine.write_shell_stdout(&mut io, &bytes)? {
                        display.extend(bytes);
                    }
                    return Ok(Command::Print(display));
                }
                Command::WaitForInput(prompt) => {
                    if !self.machine.write_shell_stdout(&mut io, &prompt)? {
                        display.extend(prompt);
                    }
                    command = loop {
                        match self.machine.read_shell_stdin(&mut io)? {
                            ShellInput::Console => {
                                self.pending_redirects = Some(io);
                                return Ok(Command::WaitForInput(display));
                            }
                            ShellInput::Eof => {
                                return Err(
                                    "redirected input ended before the command completed".into()
                                );
                            }
                            ShellInput::Byte(key) => {
                                if std::mem::take(&mut skip_lf) && key == b'\n' {
                                    continue;
                                }
                                skip_lf = key == b'\r' && self.line_input_waiting();
                                if let Some(next) = self.resume_input_unredirected(key)? {
                                    break next;
                                }
                            }
                        }
                    };
                }
                Command::Prompt if !display.is_empty() => return Ok(Command::Print(display)),
                _ => return Ok(command),
            }
        }
    }
}
