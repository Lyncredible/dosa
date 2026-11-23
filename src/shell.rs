use std::collections::BTreeMap;
use std::error::Error;

use dosa_dos::Machine;
use dosa_mount::{Entry, Mount};

mod batch;

use batch::Batch;
pub use batch::BatchStep;

pub enum Command {
    Print(Vec<u8>),
    Launched,
    Clear,
    Prompt,
    Exit,
}

pub struct Shell {
    pub machine: Machine,
    drive: char,
    directories: BTreeMap<char, String>,
    batch: Option<Batch>,
    errorlevel: u8,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        Self {
            machine: Machine::new(),
            drive: 'C',
            directories: BTreeMap::new(),
            batch: None,
            errorlevel: 0,
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), Box<dyn Error>> {
        let select = self.directories.is_empty();
        self.machine.mount(drive, mount)?;
        self.directories.insert(drive, String::new());
        if select {
            self.drive = drive;
        }
        Ok(())
    }

    pub fn prompt(&self) -> String {
        let directory = self.directories.get(&self.drive).map_or("", String::as_str);
        format!("{}:\\{directory}> ", self.drive)
    }

    pub fn current_drive(&self) -> char {
        self.drive
    }

    pub fn execute(&mut self, line: &str) -> Result<Command, Box<dyn Error>> {
        let (word, args) = split_word(line.trim())?;
        let output = match word.to_ascii_uppercase().as_str() {
            "" => return Ok(Command::Prompt),
            "CLS" => return Ok(Command::Clear),
            "EXIT" => return Ok(Command::Exit),
            "ECHO" => format!("{args}\r\n").into_bytes(),
            "REM" => return Ok(Command::Prompt),
            "HELP" => b"DIR [path]   List files\r\n\
                CD [path]    Show or change directory\r\n\
                TYPE file    Print a file\r\n\
                MOUNT C path Mount a host file, folder or ZIP\r\n\
                C:           Switch drives\r\n\
                CLS          Clear the screen\r\n\
                EXIT         Close dosa\r\n\
                Enter a COM, EXE, or BAT name and arguments to run it.\r\n"
                .to_vec(),
            "DIR" => {
                let (drive, path) = self.resolve(unquote(args))?;
                let mount = self.drive_mount(drive)?;
                let entries = match mount.list(&path) {
                    Ok(entries) => entries,
                    Err(error) => match mount.metadata(&path) {
                        Ok(entry) if !entry.is_dir => vec![entry],
                        _ => return Err(error.into()),
                    },
                };
                let mut text = format!(" Directory of {drive}:\\{path}\r\n\r\n");
                for Entry {
                    name, is_dir, size, ..
                } in entries
                {
                    if is_dir {
                        text.push_str(&format!("{name:<14} <DIR>\r\n"));
                    } else {
                        text.push_str(&format!("{name:<14} {size:>8}\r\n"));
                    }
                }
                text.into_bytes()
            }
            "CD" | "CHDIR" => {
                if args.is_empty() {
                    let prompt = self.prompt();
                    format!("{}\r\n", prompt.trim_end_matches("> ")).into_bytes()
                } else {
                    let (drive, path) = self.resolve(unquote(args))?;
                    if !path.is_empty() && !self.drive_mount(drive)?.metadata(&path)?.is_dir {
                        return Err("path is not a directory".into());
                    }
                    self.drive_mount(drive)?;
                    self.directories.insert(drive, path);
                    return Ok(Command::Prompt);
                }
            }
            "TYPE" => {
                let (drive, path) = self.resolve(unquote(args))?;
                self.drive_mount(drive)?.read(&path)?
            }
            "MOUNT" => {
                let (letter, host) = split_word(args)?;
                let drive = letter
                    .trim_end_matches(':')
                    .parse::<char>()
                    .ok()
                    .filter(char::is_ascii_alphabetic)
                    .ok_or("usage: MOUNT C host-path")?
                    .to_ascii_uppercase();
                if host.is_empty() {
                    return Err("usage: MOUNT C host-path".into());
                }
                self.mount(drive, Mount::open(unquote(host))?)?;
                format!("Drive {drive}: mounted\r\n").into_bytes()
            }
            _ => {
                if args.is_empty() && word.len() == 2 && word.ends_with(':') {
                    let (drive, _) = self.resolve(word)?;
                    self.drive_mount(drive)?;
                    self.drive = drive;
                    return Ok(Command::Prompt);
                }
                let (drive, path) = self.resolve(word)?;
                let executable = self.find_program(drive, path)?;
                if batch::is_batch(&executable) {
                    self.launch_batch(drive, &executable, word, args, false)?;
                    return Ok(Command::Prompt);
                }
                self.machine.launch(drive, &executable, args)?;
                return Ok(Command::Launched);
            }
        };
        Ok(Command::Print(output))
    }

    fn find_program(&self, drive: char, path: String) -> Result<String, Box<dyn Error>> {
        let mount = self.drive_mount(drive)?;
        if path
            .rsplit('\\')
            .next()
            .is_some_and(|name| name.contains('.'))
        {
            return Ok(path);
        }
        for extension in ["COM", "EXE", "BAT"] {
            let candidate = format!("{path}.{extension}");
            match mount.metadata(&candidate) {
                Ok(entry) if !entry.is_dir => return Ok(candidate),
                Ok(_) => {}
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(format!("program not found: {path}").into())
    }

    fn drive_mount(&self, drive: char) -> Result<&Mount, Box<dyn Error>> {
        self.machine
            .drive(drive)
            .ok_or_else(|| format!("drive {drive}: is not mounted").into())
    }

    fn resolve(&self, path: &str) -> Result<(char, String), Box<dyn Error>> {
        let bytes = path.as_bytes();
        let (drive, path) = if bytes.get(1) == Some(&b':') {
            if !bytes[0].is_ascii_alphabetic() {
                return Err("invalid drive letter".into());
            }
            (char::from(bytes[0].to_ascii_uppercase()), &path[2..])
        } else {
            (self.drive, path)
        };
        let base = self.directories.get(&drive).map_or("", String::as_str);
        let mut components: Vec<&str> = if path.starts_with(['\\', '/']) || base.is_empty() {
            Vec::new()
        } else {
            base.split('\\').collect()
        };
        for part in path.split(['\\', '/']) {
            match part {
                "" | "." => {}
                ".." => {
                    components.pop();
                }
                part => components.push(part),
            }
        }
        Ok((drive, components.join("\\")))
    }
}

fn split_word(line: &str) -> Result<(&str, &str), &'static str> {
    if let Some(quoted) = line.strip_prefix('"') {
        let (word, rest) = quoted.split_once('"').ok_or("unterminated quoted path")?;
        return Ok((word, rest.trim_start()));
    }
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    Ok((&line[..end], line[end..].trim_start()))
}

fn unquote(path: &str) -> &str {
    path.strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(path)
}
