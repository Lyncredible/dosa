use crate::{
    Entry, MAX_DEPTH, MountError, Result, lock,
    names::{Child, assign_aliases, find_child, valid_component},
    read_bounded,
};
use cap_std::fs::Dir;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    sync::Mutex,
};
use zip::ZipArchive;

const MAX_NODES: usize = 100_000;
const ZIP_COPY_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default)]
enum PendingNode {
    #[default]
    ImplicitDirectory,
    Directory(BTreeMap<String, PendingNode>),
    File {
        index: usize,
        size: u64,
    },
}

#[derive(Debug)]
enum Node {
    Directory(Vec<Child<Node>>),
    File(usize),
}

impl PendingNode {
    fn insert(&mut self, path: &[&str], leaf: Self, count: &mut usize) -> Result<()> {
        let Some((head, tail)) = path.split_first() else {
            // A directory may have been inferred from a previously seen descendant.
            if matches!(self, Self::ImplicitDirectory) || matches!(leaf, Self::File { .. }) {
                *self = leaf;
            }
            return Ok(());
        };
        if !matches!(self, Self::Directory(_)) {
            *self = Self::Directory(BTreeMap::new());
        }
        let Self::Directory(children) = self else {
            unreachable!("directory created above");
        };
        let child = match children.entry((*head).to_owned()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                if *count == MAX_NODES {
                    return Err(MountError::ArchiveTooLarge);
                }
                *count += 1;
                entry.insert(Self::ImplicitDirectory)
            }
        };
        // Prefer a directory over a file when both occupy the same name.
        if tail.is_empty() && matches!(child, Self::Directory(_)) {
            return Ok(());
        }
        child.insert(tail, leaf, count)
    }

    fn finalize(self, name: String) -> Child<Node> {
        let (item, is_dir, size) = match self {
            Self::ImplicitDirectory => (Node::Directory(Vec::new()), true, 0),
            Self::Directory(children) => {
                let mut children: Vec<_> = children
                    .into_iter()
                    .map(|(name, node)| node.finalize(name))
                    .collect();
                assign_aliases(&mut children);
                (Node::Directory(children), true, 0)
            }
            Self::File { index, size } => (Node::File(index), false, size),
        };
        Child::new(Entry::new(name, is_dir, size), item)
    }
}

#[derive(Debug)]
pub(crate) struct ZipMount {
    archive: Mutex<ZipArchive<File>>,
    root: Child<Node>,
}

impl ZipMount {
    pub fn new(file: File) -> Result<Self> {
        let mut archive = ZipArchive::new(file)?;
        if archive.len() > MAX_NODES {
            return Err(MountError::ArchiveTooLarge);
        }
        let mut index = PendingNode::default();
        let mut count = 0;
        for id in 0..archive.len() {
            let file = archive.by_index_raw(id)?;
            let name = file.name()?;
            if file.is_symlink() || name.starts_with(['/', '\\']) {
                continue;
            }
            let path: Vec<_> = name
                .trim_end_matches(['/', '\\'])
                .split(['/', '\\'])
                .collect();
            if path.len() > MAX_DEPTH || !path.iter().all(|component| valid_component(component)) {
                continue;
            }
            let node = if file.is_dir() {
                PendingNode::Directory(BTreeMap::new())
            } else {
                PendingNode::File {
                    index: id,
                    size: file.size(),
                }
            };
            index.insert(&path, node, &mut count)?;
        }
        Ok(Self {
            archive: Mutex::new(archive),
            root: index.finalize(String::new()),
        })
    }

    fn entry(&self, components: &[&str], path: &str) -> Result<&Child<Node>> {
        let mut node = &self.root;
        for component in components {
            let Node::Directory(children) = &node.item else {
                return Err(MountError::NotDirectory(path.to_owned()));
            };
            node = &children[find_child(children, component, path)?];
        }
        Ok(node)
    }

    pub fn list(&self, components: &[&str], path: &str) -> Result<Vec<Entry>> {
        let Node::Directory(children) = &self.entry(components, path)?.item else {
            return Err(MountError::NotDirectory(path.to_owned()));
        };
        Ok(children.iter().map(|child| child.entry.clone()).collect())
    }

    pub fn metadata(&self, components: &[&str], path: &str) -> Result<Entry> {
        Ok(self.entry(components, path)?.entry.clone())
    }

    pub fn read(&self, components: &[&str], path: &str) -> Result<Vec<u8>> {
        let child = self.entry(components, path)?;
        let Node::File(index) = child.item else {
            return Err(MountError::IsDirectory(path.to_owned()));
        };
        self.read_file(index, child.entry.size)
    }

    pub fn copy_to(&self, root: &Dir) -> Result<()> {
        let mut remaining = ZIP_COPY_LIMIT;
        self.copy_node(&self.root.item, root, &mut remaining)
    }

    fn copy_node(&self, node: &Node, target: &Dir, remaining: &mut u64) -> Result<()> {
        let Node::Directory(children) = node else {
            unreachable!("ZIP copy starts at a directory");
        };
        for child in children {
            let name = &child.entry.name;
            match &child.item {
                Node::Directory(_) => {
                    target.create_dir(name)?;
                    self.copy_node(&child.item, &target.open_dir(name)?, remaining)?;
                }
                Node::File(index) => {
                    let size = child.entry.size;
                    *remaining =
                        remaining
                            .checked_sub(size)
                            .ok_or(MountError::ZipCopyTooLarge {
                                limit: ZIP_COPY_LIMIT,
                            })?;
                    let data = self.read_file(*index, size)?;
                    target.create(name)?.write_all(&data)?;
                }
            }
        }
        Ok(())
    }

    fn read_file(&self, index: usize, size: u64) -> Result<Vec<u8>> {
        let mut archive = lock(&self.archive)?;
        let file = archive.by_index(index)?;
        let data = read_bounded(file.take(size.saturating_add(1)), size)?;
        if data.len() as u64 != size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ZIP entry length differs from its directory record",
            )
            .into());
        }
        Ok(data)
    }
}
