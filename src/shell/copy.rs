use std::collections::{BTreeMap, VecDeque};
use std::error::Error;

use dosa_dos::matches_dos_pattern;
use dosa_mount::{MountError, WritableFile, WriteDisposition};

use super::{Command, Confirmation, Shell, rename::destination_name, split_word};

const USAGE: &str = "usage: COPY [/Y|/-Y] [/A|/B] source [+ source ...] [destination] [/V]";
pub(super) const HELP: &str = "COPY [/Y|/-Y] [/A|/B] source [+ source ...] [destination] [/V]\r\n\
    Copy files or combine sources. Use /A for text and /B for binary data.\r\n\
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
    drive: char,
    parent: String,
    name: String,
    host_name: String,
    mode: Option<Mode>,
}

struct Job {
    inputs: Vec<Source>,
    path: String,
}

pub(super) struct CopyOperation {
    drive: char,
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
        let (drive, dest) = if implicit_append {
            (first.drive, in_directory(&first.parent, &first.name))
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
            drive,
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

    fn copy_sources(&self, operand: Operand<'_>) -> Result<Vec<Source>, Box<dyn Error>> {
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
                drive,
                parent: parent.clone(),
                name: entry.name,
                host_name: entry.host_name,
                mode: operand.mode,
            })
            .collect())
    }
}

impl CopyOperation {
    fn advance(
        mut self,
        shell: &mut Shell,
        mut output: Vec<u8>,
    ) -> Result<Command, Box<dyn Error>> {
        while let Some(job) = self.jobs.front() {
            let mount = shell.drive_mount(self.drive)?;
            let existing = match mount.metadata(&job.path) {
                Ok(entry) => {
                    let parent = split_path(&job.path).0;
                    if job.inputs.len() == 1
                        && job.inputs[0].drive == self.drive
                        && job.inputs[0].parent.eq_ignore_ascii_case(parent)
                        && job.inputs[0].name.eq_ignore_ascii_case(&entry.name)
                    {
                        return Err("File cannot be copied onto itself".into());
                    }
                    Some(mount.open_writable_file(&job.path, WriteDisposition::OpenExisting)?)
                }
                Err(error) if error.is_not_found() => None,
                Err(error) => return Err(error.into()),
            };
            if existing.is_some() && !self.overwrite {
                output.extend_from_slice(
                    format!("Overwrite {}:\\{} (Y/N)? ", self.drive, job.path).as_bytes(),
                );
                self.pending = existing;
                shell.confirmation = Some(Confirmation::Copy(self));
                return Ok(Command::Confirm(output));
            }
            let job = self.jobs.pop_front().expect("a copy job is available");
            self.write(shell, job, existing, &mut output)?;
        }
        output.extend_from_slice(format!("{} file(s) copied.\r\n", self.copied).as_bytes());
        Ok(Command::Print(output))
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
        for source in job.inputs {
            let mount = shell.drive_mount(source.drive)?;
            let path = in_directory(&source.parent, &source.host_name);
            let entry = mount.metadata(&path)?;
            if entry.host_name != source.host_name {
                return Err(MountError::NotFound(path).into());
            }
            let mut bytes = mount.read(&path)?;
            let ascii = source
                .mode
                .map_or(combining || self.default_ascii, |mode| mode == Mode::Ascii);
            if ascii && let Some(eof) = bytes.iter().position(|&byte| byte == 0x1a) {
                bytes.truncate(eof);
            }
            if data.len() + bytes.len() > dosa_mount::MAX_FILE_BYTES as usize {
                return Err(MountError::FileTooLarge.into());
            }
            data.extend(bytes);
        }
        if self.destination_mode == Some(Mode::Ascii) {
            if data.len() == dosa_mount::MAX_FILE_BYTES as usize {
                return Err(MountError::FileTooLarge.into());
            }
            data.push(0x1a);
        }
        let mount = shell.drive_mount(self.drive)?;
        if let Some(file) = existing {
            file.replace(&data)?;
            if self.verify && file.read()? != data {
                return Err("COPY verification failed".into());
            }
        } else {
            mount.write_new_file(&job.path, &data)?;
            if self.verify && mount.read(&job.path)? != data {
                return Err("COPY verification failed".into());
            }
        }
        if combining || !self.jobs.is_empty() || self.copied != 0 {
            output.extend_from_slice(format!("{}:\\{}\r\n", self.drive, job.path).as_bytes());
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
