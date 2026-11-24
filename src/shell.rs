use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

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
    quick_launch: Option<QuickLaunch>,
}

struct QuickLaunch {
    drive: char,
    previous_drive: char,
    previous_directories: BTreeMap<char, String>,
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
            quick_launch: None,
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), Box<dyn Error>> {
        let drive = drive.to_ascii_uppercase();
        let select = self.directories.is_empty();
        self.machine.mount(drive, mount)?;
        self.directories.insert(drive, String::new());
        if select {
            self.drive = drive;
        }
        Ok(())
    }

    pub(crate) fn unmount(&mut self, drive: char) -> Result<(), dosa_dos::DosError> {
        let drive = drive.to_ascii_uppercase();
        self.machine.unmount(drive)?;
        self.directories.remove(&drive);
        if self.drive == drive {
            self.drive = self.directories.keys().next().copied().unwrap_or('C');
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

    pub(crate) fn quick_launch(&mut self, path: &Path) -> Result<Command, Box<dyn Error>> {
        if self.batch_active() || self.quick_launch.is_some() {
            return Err(dosa_dos::DosError::Busy.into());
        }
        let drive = ('A'..='Z')
            .rev()
            .find(|&drive| self.machine.drive(drive).is_none())
            .ok_or("quick launch requires an unused drive letter")?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid program filename")?;
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["COM", "EXE", "BAT"]
                    .iter()
                    .any(|kind| extension.eq_ignore_ascii_case(kind))
            });
        if !supported {
            return Err("select a COM, EXE, or BAT file".into());
        }
        let folder = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mount = Mount::open(folder)?;
        let program = mount
            .list("")?
            .into_iter()
            .find(|entry| !entry.is_dir && entry.host_name == filename)
            .ok_or("selected program is not available in its folder")?;
        let scope = QuickLaunch {
            drive,
            previous_drive: self.drive,
            previous_directories: self.directories.clone(),
        };
        self.mount(drive, mount)?;
        self.drive = drive;
        self.quick_launch = Some(scope);
        match self.execute(&format!("{drive}:\\{}", program.name)) {
            Ok(command) => Ok(command),
            Err(error) => {
                self.finish_quick_launch()?;
                Err(error)
            }
        }
    }

    pub(crate) fn finish_quick_launch(&mut self) -> Result<(), dosa_dos::DosError> {
        if let Some(scope) = &self.quick_launch {
            self.machine.unmount(scope.drive)?;
            let scope = self.quick_launch.take().unwrap();
            self.drive = scope.previous_drive;
            self.directories = scope.previous_directories;
        }
        Ok(())
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
                IMGMOUNT D path Mount an ISO 9660 data image\r\n\
                Start dosa with --mount-rw C=path for a writable directory.\r\n\
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
            command @ ("MOUNT" | "IMGMOUNT") => {
                let usage = if command == "IMGMOUNT" {
                    "usage: IMGMOUNT D host-iso"
                } else {
                    "usage: MOUNT C host-path"
                };
                let (letter, host) = split_word(args)?;
                let drive = letter
                    .trim_end_matches(':')
                    .parse::<char>()
                    .ok()
                    .filter(char::is_ascii_alphabetic)
                    .ok_or(usage)?
                    .to_ascii_uppercase();
                if host.is_empty() {
                    return Err(usage.into());
                }
                let mount = if command == "IMGMOUNT" {
                    Mount::open_iso_image(unquote(host))?
                } else {
                    Mount::open(unquote(host))?
                };
                self.mount(drive, mount)?;
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
