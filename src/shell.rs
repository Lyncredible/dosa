use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use dosa_dos::{BootDrive, CodePage, DOS_VERSION, Machine, ShellIo};
use dosa_mount::Mount;

mod batch;
mod clock;
mod condition;
mod copy;
mod delete;
mod directory;
mod for_loop;
mod pending_input;
mod pipeline;
mod prompt;
mod redirection;
mod rename;
mod search_path;
mod snapshot;
mod volume;

use batch::Batch;
pub use batch::{BatchStep, PAUSE_MESSAGE};
use pending_input::PendingInput;
pub(crate) use snapshot::MAX_DESKTOP_SNAPSHOT_BYTES;

const MOUNT_HELP: &str = "MOUNT drive [/ISO | /FAT | /RW | /SESSION] host-path\r\n\
    Files, directories, and ZIPs open read-only by default.\r\n\
    /ISO      Mount an ISO 9660 data image.\r\n\
    /FAT      Mount a FAT volume or partitioned disk image.\r\n\
    /RW       Mount a writable host directory.\r\n\
    /SESSION  Mount a temporary writable copy of a ZIP.\r\n";
const MKDIR_HELP: &str = "MKDIR [drive:]path\r\nMD [drive:]path\r\nCreate a directory. The parent directory must exist.\r\n";
const RMDIR_HELP: &str = "RMDIR [drive:]path\r\nRD [drive:]path\r\nRemove an empty directory. Change out of it first.\r\n";
const SET_HELP: &str =
    "SET [variable=[value]]\r\nList, set, or clear DOS environment variables.\r\n";

const CHCP_HELP: &str = "CHCP [number]\r\nShow or change the DOS console code page. Available pages are 437 and 850.\r\n";
const BREAK_HELP: &str = "BREAK [ON | OFF]\r\nShow or set extended Ctrl+C checking.\r\n";
const VER_HELP: &str = "VER\r\nDisplay the DOS version reported to programs.\r\n";
const VERIFY_HELP: &str =
    "VERIFY [ON | OFF]\r\nShow or set read-back verification of file writes.\r\n";

pub enum Command {
    Print(Vec<u8>),
    Launched,
    Clear,
    Pause,
    WaitForInput(Vec<u8>),
    Prompt,
    Exit,
}

pub struct Shell {
    pub machine: Machine,
    drive: char,
    directories: BTreeMap<char, String>,
    batch: Option<Batch>,
    pending_input: Option<PendingInput>,
    pending_redirects: Option<ShellIo>,
    pipelines: Vec<pipeline::Pipeline>,
    errorlevel: u8,
    quick_launch: Option<QuickLaunch>,
}

#[derive(serde::Serialize, serde::Deserialize)]
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
            pending_input: None,
            pending_redirects: None,
            pipelines: Vec::new(),
            errorlevel: 0,
            quick_launch: None,
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), Box<dyn Error>> {
        if self.input_waiting() {
            return Err(dosa_dos::DosError::Busy.into());
        }
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
        if self.input_waiting() {
            return Err(dosa_dos::DosError::Busy);
        }
        let drive = drive.to_ascii_uppercase();
        self.machine.unmount(drive)?;
        self.directories.remove(&drive);
        if self.drive == drive {
            self.drive = self.directories.keys().next().copied().unwrap_or('C');
        }
        Ok(())
    }

    pub(crate) fn command_active(&self) -> bool {
        self.batch_active() || self.input_waiting() || !self.pipelines.is_empty()
    }

    pub fn current_drive(&self) -> char {
        self.drive
    }

    pub(crate) fn reset(&mut self) -> Result<(), dosa_dos::DosError> {
        self.batch = None;
        self.pending_input = None;
        self.pending_redirects = None;
        self.pipelines.clear();
        self.errorlevel = 0;
        self.machine.reset();
        self.finish_quick_launch()?;
        for directory in self.directories.values_mut() {
            directory.clear();
        }
        Ok(())
    }

    pub(crate) fn quick_launch(&mut self, path: &Path) -> Result<Command, Box<dyn Error>> {
        if self.command_active() || self.quick_launch.is_some() {
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
        if self.input_waiting() {
            return Err(dosa_dos::DosError::Busy.into());
        }
        let (word, _) = split_word(line.trim_start())?;
        if !word.eq_ignore_ascii_case("FOR") {
            if let Some(stages) = pipeline::parse(line)? {
                return self
                    .start_pipeline(stages, false)
                    .map_err(|error| self.guest_failed(error));
            }
            if let Some(request) = redirection::parse(line)? {
                return self.execute_redirected(request, false, None);
            }
        }
        self.execute_command(line, &mut None)
    }

    fn execute_command(
        &mut self,
        line: &str,
        io: &mut Option<ShellIo>,
    ) -> Result<Command, Box<dyn Error>> {
        let (word, args) = split_word(line.trim_start())?;
        if word.eq_ignore_ascii_case("SET") {
            return self.set_variable(args);
        }
        if word.eq_ignore_ascii_case("PROMPT") {
            return self.set_prompt(args);
        }
        if let Some(path) = line.trim_start().get(4..)
            && word
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("PATH"))
            && path.starts_with([';', '='])
        {
            return self.path_command(path.trim_end());
        }
        let args = args.trim_end();
        if let Some((command, _)) = word.split_once('/')
            && command.eq_ignore_ascii_case("DIR")
        {
            return self.directory(&line.trim()[command.len()..]);
        }
        let output = match word.to_ascii_uppercase().as_str() {
            "" => return Ok(Command::Prompt),
            "CLS" => return Ok(Command::Clear),
            "EXIT" => {
                self.pipelines.clear();
                return Ok(Command::Exit);
            }
            "ECHO" => self.machine.code_page().encode_text(&format!("{args}\r\n")),
            "REM" => return Ok(Command::Prompt),
            "IF" if args == "/?" => condition::HELP.as_bytes().to_vec(),
            "FOR" if args == "/?" => for_loop::HELP.as_bytes().to_vec(),
            "FOR" => return self.start_prompt_loop(args),
            "SHIFT" if args == "/?" => batch::SHIFT_HELP.as_bytes().to_vec(),
            "PAUSE" if args == "/?" => batch::PAUSE_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("MOUNT") => MOUNT_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("SET") => SET_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("PATH") => search_path::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("IF") => condition::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("FOR") => for_loop::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("SHIFT") => batch::SHIFT_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("PAUSE") => batch::PAUSE_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("PROMPT") => prompt::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("MD") || args.eq_ignore_ascii_case("MKDIR") => {
                MKDIR_HELP.as_bytes().to_vec()
            }
            "HELP" if args.eq_ignore_ascii_case("RD") || args.eq_ignore_ascii_case("RMDIR") => {
                RMDIR_HELP.as_bytes().to_vec()
            }
            "HELP" if args.eq_ignore_ascii_case("DATE") => clock::DATE_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("TIME") => clock::TIME_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("CHCP") => CHCP_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("BREAK") => BREAK_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("VER") => VER_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("VERIFY") => VERIFY_HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("VOL") => volume::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("DIR") => directory::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("COPY") => copy::HELP.as_bytes().to_vec(),
            "HELP" if args.eq_ignore_ascii_case("DEL") || args.eq_ignore_ascii_case("ERASE") => {
                delete::HELP.as_bytes().to_vec()
            }
            "HELP" if args.eq_ignore_ascii_case("REN") || args.eq_ignore_ascii_case("RENAME") => {
                rename::HELP.as_bytes().to_vec()
            }
            "HELP" => b"DIR [path]                 List files (see HELP DIR)\r\n\
                CD [path]                  Show or change directory\r\n\
                MD path                    Make a directory (MKDIR)\r\n\
                RD path                    Remove an empty directory (RMDIR)\r\n\
                REN source name            Rename files (RENAME)\r\n\
                COPY source [destination]  Copy or combine files\r\n\
                DEL path [/P]              Delete files (ERASE)\r\n\
                TYPE file                  Print a file\r\n\
                SET [variable=[value]]     Display or change environment variables\r\n\
                PATH [directories]         Set the executable search path\r\n\
                PROMPT [text]               Customize the DOS prompt\r\n\
                MOUNT drive path           Mount host files or images (see HELP MOUNT)\r\n\
                BOOT A|C path              Boot a floppy or hard disk image\r\n\
                DATE [mm-dd-yy]            Show or set the DOS date\r\n\
                TIME [hh:mm]               Show or set the DOS time\r\n\
                VER                        Display the DOS version\r\n\
                CHCP [number]              Show or change the DOS code page\r\n\
                BREAK [ON|OFF]             Show or set extended Ctrl+C checking\r\n\
                VERIFY [ON|OFF]            Show or set file write verification\r\n\
                VOL [drive:]               Display a drive volume label\r\n\
                C:                         Switch drives\r\n\
                CLS                        Clear the screen\r\n\
                EXIT                       Close Dosa\r\n\
                Use < file, > file, or >> file to redirect input and output. Join commands with |.\r\n\
                Enter a COM, EXE, or BAT name and arguments to run it.\r\n"
                .to_vec(),
            "DIR" => return self.directory(args),
            "DATE" => return Ok(self.clock_command(clock::Component::Date, args)),
            "TIME" => return Ok(self.clock_command(clock::Component::Time, args)),
            "VOL" => return self.volume(args),
            "VER" => match args {
                "/?" => VER_HELP.as_bytes().to_vec(),
                "" => {
                    let [major, minor] = DOS_VERSION;
                    format!("Dosa DOS version {major}.{minor:02}\r\n").into_bytes()
                }
                _ => return Err("usage: VER".into()),
            },
            "CHCP" => match args {
                "/?" => CHCP_HELP.as_bytes().to_vec(),
                "" => format!(
                    "Active code page: {}\r\n",
                    self.machine.code_page().number()
                )
                .into_bytes(),
                value => {
                    let page = value
                        .parse()
                        .ok()
                        .and_then(CodePage::from_number)
                        .ok_or("Invalid code page")?;
                    self.machine.set_code_page(page);
                    return Ok(Command::Prompt);
                }
            },
            "BREAK" => match args.to_ascii_uppercase().as_str() {
                "/?" => BREAK_HELP.as_bytes().to_vec(),
                "" => {
                    let state = if self.machine.extended_break() {
                        "on"
                    } else {
                        "off"
                    };
                    format!("BREAK is {state}\r\n").into_bytes()
                }
                "ON" | "OFF" => {
                    self.machine
                        .set_extended_break(args.eq_ignore_ascii_case("ON"));
                    return Ok(Command::Prompt);
                }
                _ => return Err("usage: BREAK [ON | OFF]".into()),
            },
            "VERIFY" => match args.to_ascii_uppercase().as_str() {
                "/?" => VERIFY_HELP.as_bytes().to_vec(),
                "" => {
                    let state = if self.machine.verify_writes() {
                        "on"
                    } else {
                        "off"
                    };
                    format!("VERIFY is {state}\r\n").into_bytes()
                }
                "ON" | "OFF" => {
                    self.machine
                        .set_verify_writes(args.eq_ignore_ascii_case("ON"));
                    return Ok(Command::Prompt);
                }
                _ => return Err("usage: VERIFY [ON | OFF]".into()),
            },
            "CD" | "CHDIR" => {
                if args.is_empty() {
                    format!("{}\r\n", self.current_path()).into_bytes()
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
            "MD" | "MKDIR" => return self.change_directory_entry(args, true),
            "RD" | "RMDIR" => return self.change_directory_entry(args, false),
            "PATH" => return self.path_command(args),
            "REN" | "RENAME" => return self.rename(args),
            "COPY" => return self.copy(args),
            "DEL" | "ERASE" => return self.delete(args),
            "MOUNT" => self.mount_command(args, false)?,
            "IMGMOUNT" => self.mount_command(args, true)?,
            "BOOT" => {
                const USAGE: &str = "usage: BOOT A|C host-disk-image";
                let (drive, host) = split_word(args)?;
                let drive = match drive.trim_end_matches(':').to_ascii_uppercase().as_str() {
                    "A" => BootDrive::Floppy,
                    "C" => BootDrive::HardDisk,
                    _ => return Err(USAGE.into()),
                };
                let host = unquote(host);
                if host.is_empty() {
                    return Err(USAGE.into());
                }
                self.machine.boot(std::fs::File::open(host)?, drive)?;
                return Ok(Command::Launched);
            }
            _ => {
                if args.is_empty() && word.len() == 2 && word.ends_with(':') {
                    let (drive, _) = self.resolve(word)?;
                    self.drive_mount(drive)?;
                    self.drive = drive;
                    return Ok(Command::Prompt);
                }
                let (drive, executable) = self.find_program(word, &["COM", "EXE", "BAT"])?;
                if batch::is_batch(&executable) {
                    self.launch_batch(drive, &executable, word, args, false, io.take())?;
                    return Ok(Command::Prompt);
                }
                if let Some(io) = io.take() {
                    self.machine
                        .launch_with_shell_io(drive, &executable, args, io)?;
                } else {
                    self.machine.launch_from_shell(
                        drive,
                        &executable,
                        args,
                        self.drive,
                        &self.directories,
                    )?;
                }
                return Ok(Command::Launched);
            }
        };
        Ok(Command::Print(output))
    }

    fn set_variable(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        if args.trim_end() == "/?" {
            return Ok(Command::Print(SET_HELP.as_bytes().to_vec()));
        }
        if args.is_empty() {
            let output = self
                .machine
                .environment()
                .variables()
                .map(|(name, value)| format!("{name}={value}\r\n"))
                .collect::<String>();
            return Ok(Command::Print(output.into_bytes()));
        }
        let (name, value) = args
            .split_once('=')
            .ok_or("usage: SET [variable=[value]]")?;
        self.machine.environment_mut().set(name, value)?;
        Ok(Command::Prompt)
    }

    fn change_directory_entry(&self, args: &str, create: bool) -> Result<Command, Box<dyn Error>> {
        let (path, rest) = split_word(args)?;
        let help = if create { MKDIR_HELP } else { RMDIR_HELP };
        if args == "/?" {
            return Ok(Command::Print(help.as_bytes().to_vec()));
        }
        if path.is_empty() || !rest.is_empty() {
            return Err(if create {
                "usage: MD [drive:]path"
            } else {
                "usage: RD [drive:]path"
            }
            .into());
        }
        let (drive, path) = self.resolve(path)?;
        let mount = self.drive_mount(drive)?;
        if create {
            mount.create_dir(&path)?;
        } else {
            if path.is_empty() {
                return Err("cannot remove the root directory".into());
            }
            let target = mount.canonical_directory(&path)?;
            let current = self.directories.get(&drive).map_or("", String::as_str);
            if mount
                .canonical_directory(current)
                .is_ok_and(|current| current == target)
            {
                return Err("cannot remove the current directory".into());
            }
            mount.remove_dir(&path)?;
        }
        Ok(Command::Prompt)
    }

    fn mount_command(&mut self, args: &str, legacy_iso: bool) -> Result<Vec<u8>, Box<dyn Error>> {
        const USAGE: &str = "usage: MOUNT drive [/ISO | /FAT | /RW | /SESSION] host-path";
        if args == "/?" {
            return Ok(MOUNT_HELP.as_bytes().to_vec());
        }
        let (letter, host) = split_word(args)?;
        let drive = letter
            .trim_end_matches(':')
            .parse::<char>()
            .ok()
            .filter(char::is_ascii_alphabetic)
            .ok_or(USAGE)?
            .to_ascii_uppercase();
        let (format, host) = if legacy_iso {
            ("/ISO".to_owned(), host)
        } else {
            let (first, rest) = split_word(host)?;
            let option = first.to_ascii_uppercase();
            if !host.starts_with('"')
                && matches!(option.as_str(), "/ISO" | "/FAT" | "/RW" | "/SESSION")
            {
                (option, rest)
            } else {
                (String::new(), host)
            }
        };
        let path = unquote(host);
        if path.is_empty() {
            return Err(USAGE.into());
        }
        let mount = match format.as_str() {
            "/ISO" => Mount::open_iso_image(path),
            "/FAT" => Mount::open_fat_image(path),
            "/RW" => Mount::open_writable_directory(path),
            "/SESSION" => Mount::open_session_zip(path),
            _ => Mount::open(path),
        }?;
        self.mount(drive, mount)?;
        Ok(format!("Drive {drive}: mounted\r\n").into_bytes())
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
