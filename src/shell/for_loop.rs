use std::error::Error;

use dosa_dos::matches_dos_pattern;
use dosa_mount::MountError;

use super::{Shell, split_word};

const USAGE: &str = "usage: FOR %variable IN (set) DO command (use %%variable in a batch file)";
pub(super) const HELP: &str = "FOR %variable IN (set) DO command\r\n\
    Run a command for each item. Use %%variable in a batch file.\r\n";

pub(super) struct ForLoop {
    variable: String,
    command: String,
    items: std::vec::IntoIter<String>,
}

impl ForLoop {
    pub(super) fn parse(shell: &Shell, args: &str, limit: usize) -> Result<Self, Box<dyn Error>> {
        let (variable, rest) = split_word(args)?;
        if !matches!(variable.as_bytes(), [b'%', b'A'..=b'Z' | b'a'..=b'z']) {
            return Err(USAGE.into());
        }
        let (keyword, rest) = split_word(rest)?;
        if !keyword.eq_ignore_ascii_case("IN") {
            return Err(USAGE.into());
        }
        let set = rest.strip_prefix('(').ok_or(USAGE)?;
        let mut quoted = false;
        let end = set
            .find(|ch| match ch {
                '"' => {
                    quoted = !quoted;
                    false
                }
                ')' => !quoted,
                _ => false,
            })
            .ok_or(USAGE)?;
        let (keyword, command) = split_word(set[end + 1..].trim_start())?;
        if !keyword.eq_ignore_ascii_case("DO") || command.is_empty() {
            return Err(USAGE.into());
        }
        let mut items = Vec::new();
        let separator = |ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '=');
        let mut set = set[..end].trim_start_matches(separator);
        while !set.is_empty() {
            let (item, remaining) = if let Some(quoted) = set.strip_prefix('"') {
                let end = quoted.find('"').ok_or(USAGE)? + 2;
                set.split_at(end)
            } else {
                set.split_at(set.find(separator).unwrap_or(set.len()))
            };
            set = remaining.trim_start_matches(separator);
            if items.len() == limit {
                return Err("FOR iteration limit exceeded".into());
            }
            if item.contains(['*', '?']) {
                shell.expand_for_pattern(item, &mut items, limit)?;
            } else {
                items.push(item.to_owned());
            }
        }
        Ok(Self {
            variable: variable.to_owned(),
            command: command.to_owned(),
            items: items.into_iter(),
        })
    }

    pub(super) fn next_command(&mut self, byte_limit: usize) -> Result<Option<String>, String> {
        let Some(item) = self.items.next() else {
            return Ok(None);
        };
        let matches = self.command.matches(&self.variable).count();
        let length = self.command.len() - matches * self.variable.len() + matches * item.len();
        if length > byte_limit {
            return Err(format!("FOR command exceeds the {byte_limit}-byte limit"));
        }
        Ok(Some(self.command.replace(&self.variable, &item)))
    }
}

impl Shell {
    fn expand_for_pattern(
        &self,
        item: &str,
        items: &mut Vec<String>,
        limit: usize,
    ) -> Result<(), Box<dyn Error>> {
        let path = super::unquote(item);
        let (drive, resolved) = self.resolve(path)?;
        let (parent, pattern) = resolved.rsplit_once('\\').unwrap_or(("", &resolved));
        let prefix = path.rfind(['\\', '/', ':']).map_or("", |end| &path[..=end]);
        let Some(mount) = self.machine.drive(drive) else {
            return Ok(());
        };
        let entries = match mount.list(parent) {
            Ok(entries) => entries,
            Err(error) if error.is_not_found() || matches!(error, MountError::NotDirectory(_)) => {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            if !entry.is_dir && matches_dos_pattern(&entry.name, pattern) {
                if items.len() == limit {
                    return Err("FOR iteration limit exceeded".into());
                }
                let name = format!("{prefix}{}", entry.name);
                items.push(if item.starts_with('"') {
                    format!("\"{name}\"")
                } else {
                    name
                });
            }
        }
        Ok(())
    }
}
