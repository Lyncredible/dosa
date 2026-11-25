use std::{error::Error, fmt::Write};

use dosa_dos::matches_dos_pattern;
use dosa_mount::Entry;

use super::{Shell, split_word};

const HELP: &str = "DIR [drive:][path][pattern] [/W] [/B] [/L]\r\n\
    /W  List five names per row.\r\n\
    /B  List names only.\r\n\
    /L  Show names in lowercase.\r\n";

#[derive(Default)]
struct Options {
    path: String,
    wide: bool,
    bare: bool,
    lowercase: bool,
    help: bool,
}

impl Options {
    fn parse(mut args: &str) -> Result<Self, Box<dyn Error>> {
        let mut options = Self::default();
        while !args.is_empty() {
            let quoted = args.starts_with('"');
            let (word, tail) = split_word(args)?;
            args = tail;
            if let Some(switches) = word.strip_prefix('/').filter(|_| !quoted) {
                for switch in switches.split('/') {
                    match switch.to_ascii_uppercase().as_str() {
                        "W" => options.wide = true,
                        "B" => options.bare = true,
                        "L" => options.lowercase = true,
                        "?" => options.help = true,
                        _ => return Err(format!("invalid DIR switch /{switch}").into()),
                    }
                }
            } else {
                if !options.path.is_empty() {
                    options.path.push(' ');
                }
                options.path.push_str(word);
            }
        }
        Ok(options)
    }
}

impl Shell {
    pub(super) fn directory(&self, args: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let options = Options::parse(args)?;
        if options.help {
            return Ok(HELP.as_bytes().to_owned());
        }
        let (drive, path) = self.resolve(&options.path)?;
        let mount = self.drive_mount(drive)?;
        let (parent, name) = path.rsplit_once('\\').unwrap_or(("", &path));
        let (entries, heading) = if name.contains(['*', '?']) {
            let entries: Vec<_> = mount
                .list(parent)?
                .into_iter()
                .filter(|entry| matches_dos_pattern(&entry.name, name))
                .collect();
            if entries.is_empty() {
                return Err("File not found".into());
            }
            (entries, parent)
        } else {
            let entries = match mount.list(&path) {
                Ok(entries) => entries,
                Err(error) => match mount.metadata(&path) {
                    Ok(entry) if !entry.is_dir => vec![entry],
                    _ => return Err(error.into()),
                },
            };
            (entries, path.as_str())
        };
        Ok(format_listing(drive, heading, entries, &options).into_bytes())
    }
}

fn format_listing(drive: char, path: &str, mut entries: Vec<Entry>, options: &Options) -> String {
    let mut text = if options.bare {
        String::new()
    } else {
        format!(" Directory of {drive}:\\{path}\r\n\r\n")
    };
    if options.lowercase {
        for entry in &mut entries {
            entry.name.make_ascii_lowercase();
        }
    }
    if options.bare {
        for entry in entries {
            let _ = write!(text, "{}\r\n", entry.name);
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
    text
}
