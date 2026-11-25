use std::error::Error;

use dosa_mount::{Mount, MountError};

use super::{Command, Shell, unquote};

pub(super) const HELP: &str = "PATH [[drive:]directory[;...]]\r\n\
    PATH ; clears the search path. The current directory is searched first.\r\n";

impl Shell {
    pub(super) fn path_command(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        let output = match args {
            "/?" => HELP.as_bytes().to_vec(),
            "" => self.machine.environment().get("PATH").map_or_else(
                || b"No Path\r\n".to_vec(),
                |path| format!("PATH={path}\r\n").into_bytes(),
            ),
            path => {
                let path = path.strip_prefix('=').map_or(path, str::trim_start);
                let path = if path == ";" { "" } else { path };
                self.machine.environment_mut().set("PATH", path)?;
                return Ok(Command::Prompt);
            }
        };
        Ok(Command::Print(output))
    }

    pub(super) fn find_program(
        &self,
        word: &str,
        extensions: &[&str],
    ) -> Result<(char, String), Box<dyn Error>> {
        let (drive, path) = self.resolve(word)?;
        let mount = self.drive_mount(drive)?;
        if let Some(executable) = in_directory(mount, &path, extensions)? {
            return Ok((drive, executable));
        }
        if !word.contains([':', '\\', '/'])
            && let Some(search) = self.machine.environment().get("PATH")
        {
            for directory in search
                .split(';')
                .map(str::trim)
                .map(unquote)
                .filter(|path| !path.is_empty())
            {
                let Ok((drive, directory)) = self.resolve(directory) else {
                    continue;
                };
                let Some(mount) = self.machine.drive(drive) else {
                    continue;
                };
                let candidate = if directory.is_empty() {
                    word.to_owned()
                } else {
                    format!("{directory}\\{word}")
                };
                if let Some(executable) = in_directory(mount, &candidate, extensions)? {
                    return Ok((drive, executable));
                }
            }
        }
        Err(format!("program not found: {word}").into())
    }
}

fn in_directory(
    mount: &Mount,
    path: &str,
    extensions: &[&str],
) -> Result<Option<String>, MountError> {
    let is_file = |candidate: &str| match mount.metadata(candidate) {
        Ok(entry) => Ok(!entry.is_dir),
        Err(error) if error.is_not_found() || matches!(error, MountError::NotDirectory(_)) => {
            Ok(false)
        }
        Err(error) => Err(error),
    };
    if path
        .rsplit('\\')
        .next()
        .is_some_and(|name| name.contains('.'))
    {
        return Ok(is_file(path)?.then(|| path.to_owned()));
    }
    for extension in extensions {
        let candidate = format!("{path}.{extension}");
        if is_file(&candidate)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}
