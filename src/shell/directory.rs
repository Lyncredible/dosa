use std::{cmp::Ordering, error::Error, fmt::Write};

use dosa_dos::matches_dos_pattern;
use dosa_mount::Entry;

use super::{Shell, split_word};

pub(super) const HELP: &str = "DIR [drive:][path][pattern] [/W] [/B] [/L] [/S] [/O[:order]]\r\n\
    /W  List five names per row.\r\n\
    /B  List names only. With /S, list full paths.\r\n\
    /L  Show names in lowercase.\r\n\
    /S  Search subdirectories.\r\n\
    /O  Sort by name (N), extension (E), size (S), or directory group (G).\r\n\
        Prefix a sort letter with - to reverse it.\r\n\
    Set DIRCMD for defaults. Use /-switch to cancel a default.\r\n";

#[derive(Default)]
struct Options {
    path: String,
    wide: bool,
    bare: bool,
    lowercase: bool,
    recursive: bool,
    order: Vec<SortKey>,
    help: bool,
}

impl Options {
    fn parse(&mut self, mut args: &str) -> Result<(), Box<dyn Error>> {
        let mut path = String::new();
        while !args.is_empty() {
            let quoted = args.starts_with('"');
            let (word, tail) = split_word(args)?;
            args = tail;
            if let Some(switches) = word.strip_prefix('/').filter(|_| !quoted) {
                for switch in switches.split('/') {
                    self.apply_switch(switch)?;
                }
            } else {
                if !path.is_empty() {
                    path.push(' ');
                }
                path.push_str(word);
            }
        }
        if !path.is_empty() {
            self.path = path;
        }
        Ok(())
    }

    fn apply_switch(&mut self, switch: &str) -> Result<(), Box<dyn Error>> {
        let keyword = switch.to_ascii_uppercase();
        let (enable, keyword) = keyword
            .strip_prefix('-')
            .map_or((true, keyword.as_str()), |rest| (false, rest));
        match keyword {
            "W" => self.wide = enable,
            "B" => self.bare = enable,
            "L" => self.lowercase = enable,
            "S" => self.recursive = enable,
            "?" if enable => self.help = true,
            "O" if !enable => self.order.clear(),
            order if enable && order.starts_with('O') => {
                self.order = SortKey::parse(order[1..].trim_start_matches(':'))?;
            }
            _ => return Err(format!("invalid DIR switch /{switch}").into()),
        }
        Ok(())
    }

    fn sort(&self, entries: &mut [Entry]) {
        if !self.order.is_empty() {
            entries.sort_by(|left, right| {
                self.order
                    .iter()
                    .map(|key| key.compare(left, right))
                    .find(|&result| result != Ordering::Equal)
                    .unwrap_or(Ordering::Equal)
            });
        }
    }
}

struct SortKey {
    field: SortField,
    descending: bool,
}

enum SortField {
    Name,
    Extension,
    Size,
    Directory,
}

impl SortKey {
    fn parse(order: &str) -> Result<Vec<Self>, Box<dyn Error>> {
        let order = if order.is_empty() { "GN" } else { order };
        let mut keys = Vec::new();
        let mut chars = order.chars();
        while let Some(key) = chars.next() {
            let descending = key == '-';
            let field = match if descending { chars.next() } else { Some(key) } {
                Some('N') => SortField::Name,
                Some('E') => SortField::Extension,
                Some('S') => SortField::Size,
                Some('G') => SortField::Directory,
                _ => return Err(format!("invalid DIR sort order {order}").into()),
            };
            keys.push(Self { field, descending });
        }
        Ok(keys)
    }

    fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        let result = match self.field {
            SortField::Name => a.name.cmp(&b.name),
            SortField::Extension => extension(a).cmp(&extension(b)),
            SortField::Size => a.size.cmp(&b.size),
            SortField::Directory => b.is_dir.cmp(&a.is_dir),
        };
        if self.descending {
            result.reverse()
        } else {
            result
        }
    }
}

fn extension(entry: &Entry) -> (u8, &str) {
    if entry.is_dir {
        (1, "")
    } else if let Some((_, suffix)) = entry.name.split_once('.') {
        (2, suffix)
    } else {
        (0, "")
    }
}

impl Shell {
    pub(super) fn directory(&self, args: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut options = Options::default();
        // Help remains available even if a session's DIRCMD contains a typo.
        if args == "/?" {
            return Ok(HELP.as_bytes().to_owned());
        }
        if let Some(defaults) = self.machine.environment().get("DIRCMD") {
            options.parse(defaults.trim())?;
        }
        options.parse(args)?;
        if options.help {
            return Ok(HELP.as_bytes().to_owned());
        }
        let (drive, path) = self.resolve(&options.path)?;
        let mount = self.drive_mount(drive)?;
        let (parent, name) = split_path(&path);
        let (root, pattern) = if name.contains(['*', '?']) {
            (parent, Some(name))
        } else {
            match mount.metadata(&path) {
                Ok(entry) if entry.is_dir => (path.as_str(), None),
                Ok(entry) if !options.recursive => {
                    let parent = mount.canonical_directory(parent)?;
                    let mut text = String::new();
                    format_listing(&mut text, drive, &parent, vec![entry], &options);
                    return Ok(text.into_bytes());
                }
                Ok(_) => (parent, Some(name)),
                Err(error) if options.recursive && error.is_not_found() => (parent, Some(name)),
                Err(error) => return Err(error.into()),
            }
        };
        // Resolve long or relative DOS paths once. Descendants use the short
        // names reported by the same mounted directory.
        let root = mount.canonical_directory(root)?;
        let mut directories = vec![root];
        let mut text = String::new();
        while let Some(directory) = directories.pop() {
            let mut entries = mount.list(&directory)?;
            options.sort(&mut entries);
            if options.recursive {
                directories.extend(
                    entries
                        .iter()
                        .rev()
                        .filter(|entry| entry.is_dir)
                        .map(|entry| join_path(&directory, &entry.name)),
                );
            }
            if let Some(pattern) = pattern {
                entries.retain(|entry| {
                    if pattern.contains(['*', '?']) {
                        matches_dos_pattern(&entry.name, pattern)
                    } else {
                        entry.name.eq_ignore_ascii_case(pattern)
                            || entry.host_name.eq_ignore_ascii_case(pattern)
                    }
                });
                if entries.is_empty() {
                    continue;
                }
            }
            format_listing(&mut text, drive, &directory, entries, &options);
        }
        if text.is_empty() && pattern.is_some() {
            return Err("File not found".into());
        }
        Ok(text.into_bytes())
    }
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('\\').unwrap_or(("", path))
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}\\{name}")
    }
}

fn format_listing(
    text: &mut String,
    drive: char,
    path: &str,
    mut entries: Vec<Entry>,
    options: &Options,
) {
    let lowercase_path;
    let path = if options.lowercase && options.recursive && options.bare {
        lowercase_path = path.to_ascii_lowercase();
        &lowercase_path
    } else {
        path
    };
    if !options.bare {
        if !text.is_empty() {
            text.push_str("\r\n");
        }
        let _ = write!(text, " Directory of {drive}:\\{path}\r\n\r\n");
    }
    if options.lowercase {
        for entry in &mut entries {
            entry.name.make_ascii_lowercase();
        }
    }
    if options.bare {
        for entry in entries {
            if options.recursive {
                let _ = write!(text, "{drive}:\\{}\r\n", join_path(path, &entry.name));
            } else {
                let _ = write!(text, "{}\r\n", entry.name);
            }
        }
    } else if options.wide {
        for row in entries.chunks(5) {
            for (index, entry) in row.iter().enumerate() {
                let label = if entry.is_dir {
                    format!("[{}]", entry.name)
                } else {
                    entry.name.clone()
                };
                if index + 1 == row.len() {
                    text.push_str(&label);
                } else {
                    let _ = write!(text, "{label:<16}");
                }
            }
            text.push_str("\r\n");
        }
    } else {
        for Entry {
            name, is_dir, size, ..
        } in entries
        {
            if is_dir {
                let _ = write!(text, "{name:<14} <DIR>\r\n");
            } else {
                let _ = write!(text, "{name:<14} {size:>8}\r\n");
            }
        }
    }
}
