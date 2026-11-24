use crate::{Entry, MAX_DEPTH, MountError, Result};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct Child<T> {
    pub entry: Entry,
    pub item: T,
}

impl<T> Child<T> {
    pub fn new(entry: Entry, item: T) -> Self {
        Self { entry, item }
    }
}

pub(crate) fn guest_components(path: &str) -> Result<Vec<&str>> {
    let mut components = Vec::new();
    for component in path.split(['/', '\\']) {
        match component {
            "" | "." => (),
            ".." if components.pop().is_some() => (),
            value if valid_component(value) && components.len() < MAX_DEPTH => {
                components.push(value);
            }
            _ => return Err(MountError::InvalidPath(path.to_owned())),
        }
    }
    Ok(components)
}

pub(crate) fn valid_component(component: &str) -> bool {
    !matches!(component, "" | "." | "..")
        && !component.chars().any(|ch| {
            ch.is_control() || matches!(ch, ':' | '/' | '\\' | '<' | '>' | '|' | '*' | '?' | '"')
        })
}

pub(crate) fn find_child<T>(children: &[Child<T>], name: &str, path: &str) -> Result<usize> {
    children
        .iter()
        .position(|child| child.entry.name.eq_ignore_ascii_case(name))
        .or_else(|| {
            children
                .iter()
                .position(|child| child.entry.host_name.eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| MountError::NotFound(path.to_owned()))
}

pub(crate) fn assign_aliases<T>(children: &mut [Child<T>]) {
    children.sort_unstable_by(|a, b| a.entry.host_name.cmp(&b.entry.host_name));
    let mut used = HashSet::new();
    for child in children.iter_mut() {
        if let Some(name) = canonical_name(&child.entry.host_name)
            && used.insert(name.clone())
        {
            child.entry.name = name;
        }
    }

    let mut counters = HashMap::new();
    for child in children
        .iter_mut()
        .filter(|child| child.entry.name.is_empty())
    {
        let (stem, extension) = split_name(&child.entry.host_name);
        let mut stem = alias_part(stem, 6);
        if stem.is_empty() {
            stem.push('_');
        }
        let extension = alias_part(extension, 3);
        let serial = counters
            .entry((stem.clone(), extension.clone()))
            .or_insert(0u64);
        loop {
            *serial += 1;
            let suffix = format!("~{serial}");
            let prefix = &stem[..stem.len().min(8usize.saturating_sub(suffix.len()))];
            let alias = assemble_name(&format!("{prefix}{suffix}"), &extension);
            if used.insert(alias.clone()) {
                child.entry.name = alias;
                break;
            }
        }
    }
    children.sort_unstable_by(|a, b| a.entry.name.cmp(&b.entry.name));
}

pub(crate) fn canonical_name(original: &str) -> Option<String> {
    let (stem, extension) = split_name(original);
    if stem.is_empty()
        || stem.len() > 8
        || extension.len() > 3
        || original.ends_with('.')
        || !stem.chars().chain(extension.chars()).all(dos_character)
    {
        return None;
    }
    let stem = stem.to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6"
        | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6"
        | "LPT7" | "LPT8" | "LPT9" => None,
        _ => Some(assemble_name(&stem, &extension.to_ascii_uppercase())),
    }
}

fn split_name(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or((name, ""))
}

fn assemble_name(stem: &str, extension: &str) -> String {
    if extension.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{extension}")
    }
}

fn alias_part(part: &str, limit: usize) -> String {
    part.chars()
        .filter(|&ch| ch != ' ' && ch != '.')
        .take(limit)
        .map(|ch| {
            if dos_character(ch) {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn dos_character(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(ch)
}
