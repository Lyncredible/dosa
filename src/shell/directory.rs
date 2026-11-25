use std::{cmp::Ordering, error::Error, fmt::Write};

use dosa_dos::{Video, matches_dos_pattern};
use dosa_mount::{Entry, attributes};

use super::{Command, PAUSE_MESSAGE, PendingInput, Shell, split_word};

pub(super) const HELP: &str = "DIR [drive:][path][pattern] [/W] [/B] [/L] [/S] [/P] [/O[:order]] [/A[:attributes]]\r\n\
    /W  List five names per row.\r\n\
    /B  List names only. With /S, list full paths.\r\n\
    /L  Show names in lowercase.\r\n\
    /S  Search subdirectories.\r\n\
    /P  Pause after each screen.\r\n\
    /O  Sort by name (N), extension (E), size (S), or directory group (G).\r\n\
        Prefix a sort letter with - to reverse it.\r\n\
    /A  Include hidden and system entries, or filter by H, S, D, A, or R.\r\n\
        Prefix an attribute with - to exclude it.\r\n\
    Set DIRCMD for defaults. Use /-switch to cancel a default.\r\n";

#[derive(Default)]
struct Options {
    path: String,
    wide: bool,
    bare: bool,
    lowercase: bool,
    recursive: bool,
    paging: bool,
    order: Vec<SortKey>,
    attributes: AttributeFilter,
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
            "P" => self.paging = enable,
            "?" if enable => self.help = true,
            "O" if !enable => self.order.clear(),
            "A" if !enable => self.attributes = AttributeFilter::default(),
            filter if enable && filter.starts_with('A') => {
                self.attributes = AttributeFilter::parse(filter[1..].trim_start_matches(':'))?;
            }
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

struct AttributeFilter {
    required: u8,
    excluded: u8,
}

impl Default for AttributeFilter {
    fn default() -> Self {
        Self {
            required: 0,
            excluded: attributes::HIDDEN | attributes::SYSTEM,
        }
    }
}

impl AttributeFilter {
    fn parse(filter: &str) -> Result<Self, Box<dyn Error>> {
        let mut required = 0;
        let mut excluded = 0;
        let mut chars = filter.chars();
        while let Some(key) = chars.next() {
            let negate = key == '-';
            let bit = match if negate { chars.next() } else { Some(key) } {
                Some('H') => attributes::HIDDEN,
                Some('S') => attributes::SYSTEM,
                Some('D') => attributes::DIRECTORY,
                Some('A') => attributes::ARCHIVE,
                Some('R') => attributes::READ_ONLY,
                _ => return Err(format!("invalid DIR attribute filter {filter}").into()),
            };
            if negate {
                excluded |= bit;
            } else {
                required |= bit;
            }
        }
        Ok(Self { required, excluded })
    }

    fn matches(&self, flags: u8) -> bool {
        flags & self.required == self.required && flags & self.excluded == 0
    }

    fn visits(&self, entry: &Entry) -> bool {
        entry.is_dir
            && entry.attributes & self.excluded & (attributes::HIDDEN | attributes::SYSTEM) == 0
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
    pub(super) fn directory(&mut self, args: &str) -> Result<Command, Box<dyn Error>> {
        let mut options = Options::default();
        // Help remains available even if a session's DIRCMD contains a typo.
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_owned()));
        }
        if let Some(defaults) = self.machine.environment().get("DIRCMD") {
            options.parse(defaults.trim())?;
        }
        options.parse(args)?;
        if options.help {
            return Ok(Command::Print(HELP.as_bytes().to_owned()));
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
                    if !options.attributes.matches(mount.dos_attributes(&entry)) {
                        return Err("File not found".into());
                    }
                    let parent = mount.canonical_directory(parent)?;
                    let mut text = String::new();
                    format_listing(&mut text, drive, &parent, vec![entry], &options);
                    return Ok(self.listing_output(text.into_bytes(), options.paging));
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
                        .filter(|entry| options.attributes.visits(entry))
                        .map(|entry| join_path(&directory, &entry.name)),
                );
            }
            entries.retain(|entry| options.attributes.matches(mount.dos_attributes(entry)));
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
        Ok(self.listing_output(text.into_bytes(), options.paging))
    }
}

impl Shell {
    fn listing_output(&mut self, bytes: Vec<u8>, paging: bool) -> Command {
        if paging {
            Pager { bytes, offset: 0 }.next(self, Vec::new())
        } else {
            Command::Print(bytes)
        }
    }
}

pub(super) struct Pager {
    bytes: Vec<u8>,
    offset: usize,
}

impl Pager {
    pub(super) fn next(mut self, shell: &mut Shell, mut output: Vec<u8>) -> Command {
        let (columns, rows) = match shell.machine.video() {
            Video::Text(frame) => (usize::from(frame.columns), usize::from(frame.rows)),
            Video::Graphics { .. } => (80, 25),
        };
        let content_rows = rows - 1;
        let mut column = 0;
        let mut used_rows = 0;
        let start = self.offset;
        for &byte in &self.bytes[start..] {
            self.offset += 1;
            match byte {
                b'\r' => column = 0,
                b'\n' => used_rows += 1,
                _ => {
                    column += 1;
                    if column == columns {
                        column = 0;
                        used_rows += 1;
                    }
                }
            }
            if used_rows == content_rows {
                break;
            }
        }
        output.extend_from_slice(&self.bytes[start..self.offset]);
        if self.offset == self.bytes.len() {
            Command::Print(output)
        } else {
            output.extend_from_slice(PAUSE_MESSAGE);
            shell.pending_input = Some(PendingInput::Page(self));
            Command::WaitForInput(output)
        }
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
