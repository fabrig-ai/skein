//! Content-addressed directory entries, without recursive walks (fake-forge.md, 2.3).
use crate::{
    Domain, Limits,
    api::{Answer, EntryKind, Error, TreeEntry, TreeOf, What},
    git::Tree,
    store::Repository,
};
use alloc::boxed::Box;
use skein_lib::{List, Map, bytes::copy_of, sha256::Sha256};

// A prefix is a file's ordered position and the byte after a directory slash.
// A zero length names the root, including when the tree has no files.
type Prefix = (u32, u32);
type Directories = Map<Prefix, [u8; 20]>;

pub(crate) fn read(
    domain: &Domain,
    repository: &Repository,
    limits: &Limits,
    object: TreeOf,
    page: u32,
    limit: u32,
) -> Result<Answer, Error> {
    match object {
        TreeOf::Commit(commit) => {
            if !repository.has.contains(&commit) {
                return Err(Error::Missing(What::Commit));
            }
            let tree = &domain.object(commit).expect("retained commit").tree;
            let directories = directories(tree, limits);
            Ok(paged(entries(tree, &directories, b"", limits), page, limit, limits))
        }
        TreeOf::Tree(wanted) => {
            for &commit in &repository.has {
                let tree = &domain.object(commit).expect("retained commit").tree;
                let directories = directories(tree, limits);
                for (&key, id) in &directories {
                    if *id == wanted {
                        return Ok(paged(entries(tree, &directories, prefix(tree, key), limits), page, limit, limits));
                    }
                }
            }
            Err(Error::Missing(What::File))
        }
    }
}

fn paged(entries: Box<[TreeEntry]>, page: u32, limit: u32, limits: &Limits) -> Answer {
    let size = if limit == 0 { limits.page_size } else { limit.min(limits.page_size) };
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(size));
    let mut out = List::with_capacity(size);
    let exceeds_page = entries.len() > usize::try_from(size).expect("page size fits");
    for (index, entry) in entries.into_iter().enumerate() {
        if u64::try_from(index).expect("bounded entries") < skip {
            continue;
        }
        if out.room() == 0 {
            break;
        }
        out.push(entry).expect("page room checked");
    }
    let truncated = exceeds_page && !out.is_empty();
    Answer::GitTree { entries: out.into_boxed(), truncated }
}

fn directories(tree: &Tree, limits: &Limits) -> Directories {
    let capacity = limits
        .files
        .checked_mul(limits.name_bytes)
        .expect("directory bound admitted")
        .checked_add(1)
        .expect("root fits");
    let mut directories = Directories::with_capacity(capacity);
    directories.insert((0, 0), [0; 20]).expect("root directory");
    for (file, (path, _)) in tree.iter().enumerate() {
        for (index, &byte) in path.iter().enumerate() {
            if byte == b'/' {
                let end = index.checked_add(1).expect("bounded path");
                directories
                    .insert(
                        (
                            u32::try_from(file).expect("bounded file position"),
                            u32::try_from(end).expect("bounded prefix"),
                        ),
                        [0; 20],
                    )
                    .expect("one directory per path byte");
            }
        }
    }
    // Every child prefix is longer than its parent's. Hash deepest first.
    for length in (0..=limits.name_bytes).rev() {
        let mut hashes = List::with_capacity(tree.len().saturating_add(1));
        for (&key, _) in &directories {
            if key.1 == length {
                let children = entries(tree, &directories, prefix(tree, key), limits);
                hashes.push((key, tree_id(&children))).expect("at most one prefix of this length per file, and root");
            }
        }
        for (key, hash) in hashes.into_boxed() {
            *directories.get_mut(&key).expect("known directory") = hash;
        }
    }
    directories
}

fn entries(tree: &Tree, directories: &Directories, prefix: &[u8], limits: &Limits) -> Box<[TreeEntry]> {
    let mut out = Map::with_capacity(limits.files);
    for (path, content) in tree {
        let Some(rest) = path.strip_prefix(prefix) else { continue };
        let mut slash = None;
        for (index, &byte) in rest.iter().enumerate() {
            if byte == b'/' {
                slash = Some(index);
                break;
            }
        }
        let (name, kind, mode, object) = match slash {
            Some(index) => {
                let end = prefix.len().checked_add(index).expect("bounded path").checked_add(1).expect("slash fits");
                let name = copy_of(rest.get(..=index).expect("directory ordering key"));
                let hash = directory_id(tree, directories, path.get(..end).expect("directory prefix"));
                (name, EntryKind::Tree, 0o040_000, hash)
            }
            None => (copy_of(rest), EntryKind::Blob, 0o100_644, blob_id(content)),
        };
        out.insert(name, (kind, mode, object)).expect("at most one entry per file");
    }
    let mut entries = List::with_capacity(out.len());
    for (name, &(kind, mode, object)) in &out {
        let name = match kind {
            EntryKind::Tree => name.strip_suffix(b"/").expect("directory ordering key"),
            EntryKind::Blob => name.as_ref(),
        };
        entries.push(TreeEntry { name: copy_of(name), mode, kind, object }).expect("one entry per name");
    }
    entries.into_boxed()
}

fn blob_id(content: &[u8]) -> [u8; 20] {
    let mut hash = Sha256::new();
    hash.update(b"\x01");
    hash.update(&u64::try_from(content.len()).expect("bounded blob").to_be_bytes());
    hash.update(content);
    first(hash.finish())
}
fn tree_id(entries: &[TreeEntry]) -> [u8; 20] {
    let mut hash = Sha256::new();
    hash.update(b"\x02");
    for entry in entries {
        hash.update(&entry.mode.to_be_bytes());
        hash.update(&u64::try_from(entry.name.len()).expect("bounded entry").to_be_bytes());
        hash.update(&entry.name);
        hash.update(&entry.object);
    }
    first(hash.finish())
}
fn first(hash: [u8; 32]) -> [u8; 20] {
    let mut id = [0; 20];
    for (out, byte) in id.iter_mut().zip(hash) {
        *out = byte;
    }
    id
}

fn prefix(tree: &Tree, key: Prefix) -> &[u8] {
    if key.1 == 0 {
        return b"";
    }
    let (path, _) = tree.iter().nth(usize::try_from(key.0).expect("bounded file position")).expect("indexed file");
    path.get(..usize::try_from(key.1).expect("bounded prefix")).expect("directory prefix")
}
fn directory_id(tree: &Tree, directories: &Directories, wanted: &[u8]) -> [u8; 20] {
    for (&key, &id) in directories {
        if prefix(tree, key) == wanted {
            return id;
        }
    }
    unreachable!("each directory slash is indexed")
}
