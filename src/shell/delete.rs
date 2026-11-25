use std::collections::VecDeque;
use std::error::Error;

use dosa_dos::matches_dos_pattern;
use dosa_mount::{Entry, MountError};

use super::{Command, PendingInput, Shell, split_word};

const USAGE: &str = "usage: DEL [drive:][path]filename [/P]";
pub(super) const HELP: &str = "DEL [drive:][path]filename [/P]\r\n\
    ERASE [drive:][path]filename [/P]\r\n\
    Delete files. Use /P to confirm each deletion.\r\n";

pub(super) struct Deletion {
    drive: char,
    parent: String,
    host_parents: Vec<String>,
    files: VecDeque<Entry>,
    per_file: bool,
}

impl Shell {
    pub(super) fn delete(&mut self, mut args: &str) -> Result<Command, Box<dyn Error>> {
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        let mut path = None;
        let mut per_file = false;
        while !args.is_empty() {
            let (word, rest) = split_word(args)?;
            if word.eq_ignore_ascii_case("/P") {
                per_file = true;
            } else if path.is_some() || word.is_empty() {
                return Err(USAGE.into());
            } else {
                path = Some(word);
            }
            args = rest;
        }

        let (drive, path) = self.resolve(path.ok_or(USAGE)?)?;
        let mount = self.drive_mount(drive)?;
        if !mount.is_writable() {
            return Err(MountError::ReadOnly.into());
        }

        let explicit_pattern = path.contains(['*', '?']);
        let mut files = VecDeque::new();
        let (parent, pattern) = if explicit_pattern {
            let (parent, name) = path.rsplit_once('\\').unwrap_or(("", &path));
            (parent, name)
        } else {
            let entry = mount.metadata(&path)?;
            if entry.is_dir {
                (path.as_str(), "*.*")
            } else {
                files.push_back(entry);
                (path.rsplit_once('\\').map_or("", |(parent, _)| parent), "")
            }
        };
        if !pattern.is_empty() {
            files.extend(
                mount
                    .list(parent)?
                    .into_iter()
                    .filter(|entry| !entry.is_dir && matches_dos_pattern(&entry.name, pattern)),
            );
        }
        if files.is_empty() {
            return Err("File not found".into());
        }
        let mut resolved_parent = String::new();
        let mut host_parents = Vec::new();
        for component in parent.split('\\').filter(|part| !part.is_empty()) {
            if !resolved_parent.is_empty() {
                resolved_parent.push('\\');
            }
            resolved_parent.push_str(component);
            host_parents.push(mount.metadata(&resolved_parent)?.host_name);
        }
        let deleting = Deletion {
            drive,
            parent: parent.to_owned(),
            host_parents,
            files,
            per_file,
        };
        if per_file {
            return Ok(self.prompt_deletion(deleting));
        }
        if matches!(pattern, "*" | "*.*") {
            let label = deleting.display_path("");
            self.pending_input = Some(PendingInput::Delete(deleting));
            return Ok(Command::WaitForInput(
                format!("Delete all files in {label} (Y/N)? ").into_bytes(),
            ));
        }
        deleting.remove_all(self)?;
        Ok(Command::Prompt)
    }

    fn prompt_deletion(&mut self, deleting: Deletion) -> Command {
        let Some(entry) = deleting.files.front() else {
            return Command::Prompt;
        };
        let path = deleting.display_path(&entry.name);
        self.pending_input = Some(PendingInput::Delete(deleting));
        Command::WaitForInput(format!("Delete {path} (Y/N)? ").into_bytes())
    }
}

impl Deletion {
    pub(super) fn confirm(
        mut self,
        shell: &mut Shell,
        approved: bool,
    ) -> Result<Command, Box<dyn Error>> {
        if !self.per_file {
            if approved {
                self.remove_all(shell)?;
            }
            return Ok(Command::Prompt);
        }
        let entry = self
            .files
            .pop_front()
            .expect("one file is awaiting confirmation");
        if approved {
            self.remove(shell, &entry)?;
        }
        Ok(shell.prompt_deletion(self))
    }

    fn display_path(&self, name: &str) -> String {
        format!("{}:\\{}", self.drive, self.in_parent(name))
    }

    fn in_parent(&self, name: &str) -> String {
        if self.parent.is_empty() {
            name.to_owned()
        } else {
            format!("{}\\{name}", self.parent)
        }
    }

    fn remove(&self, shell: &Shell, entry: &Entry) -> Result<(), Box<dyn Error>> {
        let path = self.in_parent(&entry.host_name);
        let mount = shell.drive_mount(self.drive)?;
        mount.remove_listed_file(&path, &self.host_parents, &entry.host_name)?;
        Ok(())
    }

    fn remove_all(self, shell: &Shell) -> Result<(), Box<dyn Error>> {
        for entry in &self.files {
            self.remove(shell, entry)?;
        }
        Ok(())
    }
}
