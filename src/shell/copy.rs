use std::collections::{BTreeMap, VecDeque};
use std::error::Error;

use dosa_dos::{DosDevice, matches_dos_pattern};
use dosa_mount::{FileWriteOptions, MountError, WritableFile, WriteDisposition};

use super::{Command, PendingInput, Shell, rename::destination_name, split_word};

const USAGE: &str = "usage: COPY [/Y|/-Y] [/A|/B] source [+ source ...] [destination] [/V]";
pub(super) const HELP: &str = "COPY [/Y|/-Y] [/A|/B] source [+ source ...] [destination] [/V]\r\n\
    Copy files or combine sources. Use /A for text and /B for binary data.\r\n\
    CON copies from the keyboard or to the screen. End keyboard input with Ctrl+Z or F6, then Enter.\r\n\
    NUL supplies empty input or discards output.\r\n\
    /Y skips overwrite prompts, /-Y forces them, and /V verifies the write.\r\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ascii,
    Binary,
}

#[derive(Clone, Copy)]
struct Operand<'a> {
    path: &'a str,
    mode: Option<Mode>,
}

struct Request<'a> {
    sources: Vec<Operand<'a>>,
    destination: Option<Operand<'a>>,
    overwrite: Option<bool>,
    verify: bool,
}

impl<'a> Request<'a> {
    fn parse(mut args: &'a str) -> Result<Self, Box<dyn Error>> {
        let mut request = Self {
            sources: Vec::new(),
            destination: None,
            overwrite: None,
            verify: false,
        };
        let mut mode = None;
        let mut after_plus = false;
        while !args.is_empty() {
            if let Some(rest) = args.strip_prefix('+') {
                if request.sources.is_empty() || request.destination.is_some() || after_plus {
                    return Err(USAGE.into());
                }
                args = rest.trim_start();
                after_plus = true;
                continue;
            }
            let quoted = args.starts_with('"');
            let (word, rest) = if quoted {
                split_word(args)?
            } else {
                let end = args
                    .find(|ch: char| ch == '+' || ch.is_whitespace())
                    .unwrap_or(args.len());
                (&args[..end], args[end..].trim_start())
            };
            if word.is_empty() {
                return Err(USAGE.into());
            }
            args = rest;
            match (!quoted).then(|| word.to_ascii_uppercase()).as_deref() {
                Some("/Y") => request.overwrite = Some(true),
                Some("/-Y") => request.overwrite = Some(false),
                Some("/V") => request.verify = true,
                Some("/A" | "/B") => {
                    mode = Some(if word.eq_ignore_ascii_case("/A") {
                        Mode::Ascii
                    } else {
                        Mode::Binary
                    });
                    if let Some(destination) = request.destination.as_mut() {
                        destination.mode = mode;
                    } else if !after_plus && let Some(last) = request.sources.last_mut() {
                        last.mode = mode;
                    }
                }
                _ if request.sources.is_empty() || after_plus => {
                    request.sources.push(Operand { path: word, mode });
                    after_plus = false;
                }
                _ if request.destination.is_none() => {
                    request.destination = Some(Operand {
                        path: word,
                        mode: None,
                    });
                }
                _ => return Err(USAGE.into()),
            }
        }
        if request.sources.is_empty() || after_plus {
            return Err(USAGE.into());
        }
        Ok(request)
    }
}

struct Source {
    data: SourceData,
    name: String,
    mode: Option<Mode>,
}

enum SourceData {
    File(SourceFile),
    Null,
    ConsolePending,
    Console(Vec<u8>),
}

impl SourceData {
    fn device(&self) -> Option<DosDevice> {
        match self {
            Self::File(_) => None,
            Self::Null => Some(DosDevice::Null),
            Self::ConsolePending | Self::Console(_) => Some(DosDevice::Console),
        }
    }
}

struct SourceFile {
    drive: char,
    parent: String,
    host_name: String,
}

#[derive(Clone, Copy)]
enum Destination {
    Mounted(char),
    Device(DosDevice),
}

struct Job {
    inputs: Vec<Source>,
    path: String,
}

impl Job {
    fn console_source(&self) -> Option<usize> {
        self.inputs
            .iter()
            .position(|input| matches!(input.data, SourceData::ConsolePending))
    }
}

pub(super) struct CopyText {
    job: Job,
    existing: Option<WritableFile>,
    source: usize,
    bytes: Vec<u8>,
    remaining: usize,
    line_length: usize,
    eof: bool,
}

pub(super) struct CopyOperation {
    destination: Destination,
    jobs: VecDeque<Job>,
    pending: Option<WritableFile>,
    overwrite: bool,
    verify: bool,
    destination_mode: Option<Mode>,
    default_ascii: bool,
    copied: usize,
}

impl Shell {
    pub(super) fn copy(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        let request = Request::parse(args)?;
        let overwrite = request
            .overwrite
            .or_else(|| {
                self.machine.environment().get("COPYCMD").and_then(|value| {
                    value.split_whitespace().rev().find_map(|arg| {
                        match arg.to_ascii_uppercase().as_str() {
                            "/Y" => Some(true),
                            "/-Y" => Some(false),
                            _ => None,
                        }
                    })
                })
            })
            .unwrap_or_else(|| self.command_from_batch_file());

        let pattern_sources = request
            .sources
            .iter()
            .any(|item| item.path.contains(['*', '?']));
        let destination = request.destination;
        let groups = request
            .sources
            .into_iter()
            .map(|operand| self.copy_sources(operand))
            .collect::<Result<Vec<_>, _>>()?;
        let first = &groups[0][0];
        let implicit_append = destination.is_none() && groups.len() > 1;
        let device = if let Some(operand) = destination {
            self.copy_device_operand(operand.path)?
        } else if implicit_append {
            first.data.device()
        } else {
            None
        };
        if let Some(device) = device {
            if device == DosDevice::Console
                && groups.len() == 1
                && first.data.device() == Some(device)
            {
                return Err("File cannot be copied onto itself".into());
            }
            return CopyOperation {
                destination: Destination::Device(device),
                jobs: VecDeque::from([Job {
                    inputs: groups.into_iter().flatten().collect(),
                    path: if device == DosDevice::Console {
                        "CON"
                    } else {
                        "NUL"
                    }
                    .into(),
                }]),
                pending: None,
                overwrite: true,
                verify: false,
                destination_mode: destination.and_then(|operand| operand.mode),
                default_ascii: true,
                copied: 0,
            }
            .advance(self, Vec::new());
        }

        if destination.is_none() && first.data.device().is_some() {
            return Err("Specify a destination when copying from a device".into());
        }
        let (drive, dest) = if let (true, SourceData::File(file)) = (implicit_append, &first.data) {
            (file.drive, in_directory(&file.parent, &first.name))
        } else {
            self.resolve(destination.map_or("", |item| item.path))?
        };
        let mount = self.drive_mount(drive)?;
        if !mount.is_writable() {
            return Err(MountError::ReadOnly.into());
        }
        let directory = if dest.contains(['*', '?']) {
            false
        } else {
            match mount.metadata(&dest) {
                Ok(entry) => entry.is_dir,
                Err(error) if error.is_not_found() => false,
                Err(error) => return Err(error.into()),
            }
        };
        if !directory && destination.is_some_and(|item| item.path.ends_with(['\\', '/'])) {
            return Err(MountError::NotDirectory(dest).into());
        }
        let (parent, pattern) = if directory {
            (dest.as_str(), "*.*")
        } else {
            split_path(&dest)
        };
        let parent = mount.canonical_directory(parent)?;
        let default_ascii = groups.len() > 1 || pattern_sources && !pattern.contains(['*', '?']);
        let jobs = if !pattern.contains(['*', '?']) {
            VecDeque::from([Job {
                inputs: groups.into_iter().flatten().collect(),
                path: in_directory(&parent, pattern),
            }])
        } else if groups.len() > 1
            && !pattern_sources
            && groups.iter().all(|group| group.len() == 1)
        {
            let inputs: Vec<_> = groups.into_iter().flatten().collect();
            let name = destination_name(&inputs[0].name, pattern);
            let path = in_directory(&parent, &name);
            VecDeque::from([Job { inputs, path }])
        } else if groups.len() == 1 {
            groups
                .into_iter()
                .flatten()
                .map(|source| Job {
                    path: in_directory(&parent, &destination_name(&source.name, pattern)),
                    inputs: vec![source],
                })
                .collect()
        } else {
            let mut by_stem: BTreeMap<String, Vec<Source>> = BTreeMap::new();
            for source in groups.into_iter().flatten() {
                let stem = source.name.split('.').next().unwrap_or("").to_owned();
                by_stem.entry(stem).or_default().push(source);
            }
            by_stem
                .into_values()
                .map(|inputs| Job {
                    path: in_directory(&parent, &destination_name(&inputs[0].name, pattern)),
                    inputs,
                })
                .collect()
        };
        CopyOperation {
            destination: Destination::Mounted(drive),
            jobs,
            pending: None,
            overwrite,
            verify: request.verify,
            destination_mode: destination.and_then(|item| item.mode),
            default_ascii,
            copied: 0,
        }
        .advance(self, Vec::new())
    }

    fn copy_device_operand(&self, operand: &str) -> Result<Option<DosDevice>, Box<dyn Error>> {
        let Some(device) = DosDevice::named(operand) else {
            return Ok(None);
        };
        let (drive, path) = self.resolve(operand)?;
        let (parent, _) = split_path(&path);
        if operand.contains(':') || !parent.is_empty() {
            self.drive_mount(drive)?.canonical_directory(parent)?;
        }
        Ok(Some(device))
    }

    fn copy_sources(&self, operand: Operand<'_>) -> Result<Vec<Source>, Box<dyn Error>> {
        if let Some(device) = self.copy_device_operand(operand.path)? {
            let (data, name) = match device {
                DosDevice::Console => (SourceData::ConsolePending, "CON"),
                DosDevice::Null => (SourceData::Null, "NUL"),
            };
            return Ok(vec![Source {
                data,
                name: name.into(),
                mode: operand.mode,
            }]);
        }
        let (drive, path) = self.resolve(operand.path)?;
        let mount = self.drive_mount(drive)?;
        let (parent, pattern, single) = if path.contains(['*', '?']) {
            let (parent, pattern) = split_path(&path);
            (parent, pattern, None)
        } else {
            let entry = mount.metadata(&path)?;
            if entry.is_dir {
                (path.as_str(), "*.*", None)
            } else {
                (split_path(&path).0, "", Some(entry))
            }
        };
        let parent = mount.canonical_directory(parent)?;
        let entries = match single {
            Some(entry) => vec![entry],
            None => mount
                .list(&parent)?
                .into_iter()
                .filter(|entry| !entry.is_dir && matches_dos_pattern(&entry.name, pattern))
                .collect(),
        };
        if entries.is_empty() {
            return Err("File not found".into());
        }
        Ok(entries
            .into_iter()
            .map(|entry| Source {
                data: SourceData::File(SourceFile {
                    drive,
                    parent: parent.clone(),
                    host_name: entry.host_name,
                }),
                name: entry.name,
                mode: operand.mode,
            })
            .collect())
    }
}

impl CopyText {
    fn new(job: Job, existing: Option<WritableFile>, source: usize) -> Self {
        Self {
            job,
            existing,
            source,
            bytes: Vec::new(),
            remaining: dosa_mount::MAX_FILE_BYTES as usize,
            line_length: 0,
            eof: false,
        }
    }

    fn wait_for_key(self, copy: CopyOperation, shell: &mut Shell, echo: Vec<u8>) -> Command {
        shell.pending_input = Some(PendingInput::CopyText(copy, self));
        Command::WaitForInput(echo)
    }

    pub(super) fn key(
        mut self,
        mut copy: CopyOperation,
        shell: &mut Shell,
        key: u8,
    ) -> Result<Command, Box<dyn Error>> {
        let echo = match key {
            3 => {
                shell.errorlevel = 1;
                return Ok(Command::Print(b"^C\r\n".to_vec()));
            }
            b'\r' | b'\n' if self.eof => {
                let mut output = b"\r\n".to_vec();
                let bytes = std::mem::take(&mut self.bytes);
                self.remaining -= bytes.len();
                self.job.inputs[self.source].data = SourceData::Console(bytes);
                if let Some(source) = self.job.console_source() {
                    self.source = source;
                    self.line_length = 0;
                    self.eof = false;
                    return Ok(self.wait_for_key(copy, shell, output));
                }
                copy.write(shell, self.job, self.existing, &mut output)?;
                return copy.advance(shell, output);
            }
            8 if self.eof => {
                self.eof = false;
                b"\x08 \x08\x08 \x08".to_vec()
            }
            8 if self.line_length > 0 => {
                self.bytes.pop();
                self.line_length -= 1;
                b"\x08 \x08".to_vec()
            }
            0x1a if !self.eof => {
                self.eof = true;
                b"^Z".to_vec()
            }
            b'\r' | b'\n' => {
                self.extend(b"\r\n")?;
                self.line_length = 0;
                b"\r\n".to_vec()
            }
            b'\t' | 32..=255 if !self.eof => {
                self.extend(&[key])?;
                self.line_length += 1;
                vec![key]
            }
            _ => Vec::new(),
        };
        Ok(self.wait_for_key(copy, shell, echo))
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), MountError> {
        if bytes.len() > self.remaining - self.bytes.len() {
            return Err(MountError::FileTooLarge);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

impl CopyOperation {
    fn advance(
        mut self,
        shell: &mut Shell,
        mut output: Vec<u8>,
    ) -> Result<Command, Box<dyn Error>> {
        while let Some(job) = self.jobs.front() {
            let existing = self.open_destination(shell, job)?;
            if existing.is_some() && !self.overwrite {
                let Destination::Mounted(drive) = self.destination else {
                    unreachable!("device targets do not have overwrite confirmation");
                };
                output.extend_from_slice(
                    format!("Overwrite {drive}:\\{} (Y/N)? ", job.path).as_bytes(),
                );
                self.pending = existing;
                shell.pending_input = Some(PendingInput::Copy(self));
                return Ok(Command::WaitForInput(output));
            }
            let job = self.jobs.pop_front().expect("a copy job is available");
            if let Some(source) = job.console_source() {
                let input = CopyText::new(job, existing, source);
                return Ok(input.wait_for_key(self, shell, output));
            }
            self.write(shell, job, existing, &mut output)?;
        }
        output.extend_from_slice(format!("{} file(s) copied.\r\n", self.copied).as_bytes());
        Ok(Command::Print(output))
    }

    fn open_destination(
        &self,
        shell: &Shell,
        job: &Job,
    ) -> Result<Option<WritableFile>, Box<dyn Error>> {
        let Destination::Mounted(drive) = self.destination else {
            return Ok(None);
        };
        let mount = shell.drive_mount(drive)?;
        match mount.metadata(&job.path) {
            Ok(entry) => {
                let parent = split_path(&job.path).0;
                if job.inputs.len() == 1
                    && matches!(&job.inputs[0].data, SourceData::File(source)
                        if source.drive == drive && source.parent.eq_ignore_ascii_case(parent))
                    && job.inputs[0].name.eq_ignore_ascii_case(&entry.name)
                {
                    return Err("File cannot be copied onto itself".into());
                }
                Ok(Some(mount.open_writable_file(
                    &job.path,
                    WriteDisposition::OpenExisting,
                )?))
            }
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn confirm(
        mut self,
        shell: &mut Shell,
        approved: bool,
    ) -> Result<Command, Box<dyn Error>> {
        let pending = self
            .pending
            .take()
            .expect("destination is awaiting approval");
        let job = self
            .jobs
            .pop_front()
            .expect("a copy job is awaiting approval");
        let mut output = Vec::new();
        if approved {
            if let Some(source) = job.console_source() {
                let input = CopyText::new(job, Some(pending), source);
                return Ok(input.wait_for_key(self, shell, output));
            }
            self.write(shell, job, Some(pending), &mut output)?;
        }
        self.advance(shell, output)
    }

    fn write(
        &mut self,
        shell: &Shell,
        job: Job,
        existing: Option<WritableFile>,
        output: &mut Vec<u8>,
    ) -> Result<(), Box<dyn Error>> {
        let combining = job.inputs.len() > 1;
        let mut data = Vec::new();
        let mut modified = shell.machine.adjusted_file_time();
        for source in job.inputs {
            let mut bytes = match source.data {
                SourceData::Null => continue,
                SourceData::ConsolePending => {
                    unreachable!("console input must be collected before writing")
                }
                SourceData::Console(bytes) => bytes,
                SourceData::File(file) => {
                    let mount = shell.drive_mount(file.drive)?;
                    let path = in_directory(&file.parent, &file.host_name);
                    let entry = mount.metadata(&path)?;
                    if entry.host_name != file.host_name {
                        return Err(MountError::NotFound(path).into());
                    }
                    if !combining
                        && !self.default_ascii
                        && let Some(timestamp) = entry.modified
                    {
                        modified = Some(timestamp.to_system_time());
                    }
                    mount.read(&path)?
                }
            };
            if matches!(self.destination, Destination::Device(DosDevice::Null)) {
                continue;
            }
            let default_ascii = (combining || self.default_ascii)
                && (!matches!(self.destination, Destination::Device(_))
                    || self.destination_mode != Some(Mode::Binary));
            let ascii = source
                .mode
                .map_or(default_ascii, |mode| mode == Mode::Ascii);
            if ascii && let Some(eof) = bytes.iter().position(|&byte| byte == 0x1a) {
                bytes.truncate(eof);
            }
            if data.len() + bytes.len() > dosa_mount::MAX_FILE_BYTES as usize {
                return Err(MountError::FileTooLarge.into());
            }
            data.extend(bytes);
        }
        if matches!(self.destination, Destination::Mounted(_))
            && self.destination_mode == Some(Mode::Ascii)
        {
            if data.len() == dosa_mount::MAX_FILE_BYTES as usize {
                return Err(MountError::FileTooLarge.into());
            }
            data.push(0x1a);
        }
        let trace = combining || !self.jobs.is_empty() || self.copied != 0;
        match self.destination {
            Destination::Mounted(drive) => {
                let write = FileWriteOptions {
                    modified,
                    verify: self.verify || shell.machine.verify_writes(),
                };
                if let Some(file) = existing {
                    file.replace_with(&data, write)?;
                } else {
                    shell
                        .drive_mount(drive)?
                        .write_new_file_with(&job.path, &data, write)?;
                }
                if trace {
                    output.extend_from_slice(format!("{drive}:\\{}\r\n", job.path).as_bytes());
                }
            }
            Destination::Device(DosDevice::Console) => output.extend(data),
            Destination::Device(DosDevice::Null) if trace => output.extend_from_slice(b"NUL\r\n"),
            Destination::Device(DosDevice::Null) => {}
        }
        self.copied += 1;
        Ok(())
    }
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('\\').unwrap_or(("", path))
}

fn in_directory(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}\\{name}")
    }
}
