use std::error::Error;

use dosa_dos::matches_dos_pattern;

use super::{Command, Shell, split_word};

pub(super) const HELP: &str = "REN [drive:][path]source new-name\r\n\
    RENAME [drive:][path]source new-name\r\n\
    Rename files in the same directory. Both names accept * and ?.\r\n";

impl Shell {
    pub(super) fn rename(&self, args: &str) -> Result<Command, Box<dyn Error>> {
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        let (source, rest) = split_word(args)?;
        let (destination, extra) = split_word(rest)?;
        if source.is_empty() || destination.is_empty() || !extra.is_empty() {
            return Err("usage: REN [drive:][path]source new-name".into());
        }
        if destination.contains([':', '/', '\\']) {
            return Err("the new name must be in the same directory".into());
        }

        let (drive, path) = self.resolve(source)?;
        let (parent, pattern) = path
            .rsplit_once('\\')
            .map_or(("", path.as_str()), |(parent, name)| (parent, name));
        let mount = self.drive_mount(drive)?;
        let entries = if pattern.contains(['*', '?']) {
            mount
                .list(parent)?
                .into_iter()
                .filter(|entry| !entry.is_dir && matches_dos_pattern(&entry.name, pattern))
                .collect()
        } else {
            let entry = mount.metadata(&path)?;
            if entry.is_dir {
                return Err("REN requires a file".into());
            }
            vec![entry]
        };
        if entries.is_empty() {
            return Err("File not found".into());
        }

        let in_parent = |name: &str| {
            if parent.is_empty() {
                name.to_owned()
            } else {
                format!("{parent}\\{name}")
            }
        };
        let mut first_error = None;
        for entry in entries {
            // Short aliases can change as the directory is modified.
            let from = in_parent(&entry.host_name);
            let to = in_parent(&destination_name(&entry.name, destination));
            let result = if mount.metadata(&from)?.host_name == entry.host_name {
                mount
                    .rename_file(&from, &to)
                    .map_err(|error| error.to_string())
            } else {
                Err(format!("source alias changed for {}", entry.name))
            };
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error.into()),
            None => Ok(Command::Prompt),
        }
    }
}

pub(super) fn destination_name(source: &str, pattern: &str) -> String {
    if let Some((stem_pattern, extension_pattern)) = pattern.split_once('.') {
        let (stem, extension) = source.split_once('.').unwrap_or((source, ""));
        let mut result = substitute(stem, stem_pattern);
        let extension = substitute(extension, extension_pattern);
        if !extension.is_empty() {
            result.push('.');
            result.push_str(&extension);
        }
        result
    } else {
        substitute(source, pattern)
    }
}

fn substitute(source: &str, pattern: &str) -> String {
    let mut result = String::new();
    for (index, character) in pattern.chars().enumerate() {
        match character {
            '*' => {
                result.push_str(source.get(index..).unwrap_or(""));
                break;
            }
            '?' => result.extend(source.as_bytes().get(index).copied().map(char::from)),
            _ => result.push(character),
        }
    }
    result
}
