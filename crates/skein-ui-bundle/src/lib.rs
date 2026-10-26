//! A reproducible browser bundle (ui-web.md, 4.2).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

mod digest;
mod glue;
pub use glue::glue;
mod page;
pub mod types;
pub use digest::Digest;

/// The kit's stylesheet, `skein-ui.css`, which every bundle holds first and a
/// program printing pages beside it without a bundle copies (ui-web.md, 12).
pub const STYLESHEET: &[u8] = include_bytes!("../skein-ui.css");

/// What the application gives the bundler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub name: String,
    /// The worker module's name: its glue is `<worker>.js`, its module
    /// `<worker>_bg.wasm`, and its starter `worker.js` (playground.md, 7).
    pub worker: Option<String>,
    /// One or more segments, with one leading and one trailing slash.
    pub prefix: String,
    pub stylesheets: Vec<PathBuf>,
    /// Each owner's prefix and its pictures' directory.
    pub pictures: Vec<(String, PathBuf)>,
    pub page: Option<PathBuf>,
    pub files: Option<PathBuf>,
    pub noscript: String,
}

/// The glue generated for one module, including its imported snippets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glue {
    pub js: Vec<u8>,
    pub wasm: Vec<u8>,
    pub snippets: Vec<(String, Vec<u8>)>,
}

/// An input that could not be bundled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Type {
        file: PathBuf,
    },
    ReservedName {
        file: PathBuf,
    },
    SameName {
        file: PathBuf,
    },
    Glue,
    /// A worker named but its glue not given, or given with no name.
    Worker,
    NoMarker {
        page: PathBuf,
    },
    Io {
        path: PathBuf,
    },
}

fn io<T>(path: &Path, result: std::io::Result<T>) -> Result<T, Refusal> {
    result.map_err(|_| Refusal::Io { path: path.to_owned() })
}

/// Replace `out` with the complete bundle, returning its directory's digest.
/// `worker` is given exactly when `inputs.worker` names a worker module.
pub fn bundle(inputs: &Inputs, glue: &Glue, worker: Option<&Glue>, out: &Path) -> Result<Digest, Refusal> {
    if inputs.worker.is_some() != worker.is_some() {
        return Err(Refusal::Worker);
    }
    let mut files = BTreeMap::new();
    files.insert("boot.js".into(), page::starter(&inputs.name));
    files.insert("skein-ui.css".into(), STYLESHEET.to_vec());
    insert(&mut files, format!("{}.js", inputs.name), glue.js.clone(), Path::new(&inputs.name))?;
    insert(&mut files, format!("{}_bg.wasm", inputs.name), glue.wasm.clone(), Path::new(&inputs.name))?;
    for (path, bytes) in &glue.snippets {
        insert(&mut files, format!("snippets/{path}"), bytes.clone(), Path::new(path))?;
    }
    if let (Some(name), Some(worker)) = (&inputs.worker, worker) {
        insert(&mut files, "worker.js".into(), page::starter(name), Path::new(name))?;
        insert(&mut files, format!("{name}.js"), worker.js.clone(), Path::new(name))?;
        insert(&mut files, format!("{name}_bg.wasm"), worker.wasm.clone(), Path::new(name))?;
        for (path, bytes) in &worker.snippets {
            let name = format!("snippets/{path}");
            if files.get(&name) != Some(bytes) {
                insert(&mut files, name, bytes.clone(), Path::new(path))?;
            }
        }
    }
    for path in &inputs.stylesheets {
        let name = path.file_name().and_then(|name| name.to_str()).ok_or_else(|| Refusal::Io { path: path.clone() })?;
        if name == "skein-ui.css" {
            return Err(Refusal::ReservedName { file: path.clone() });
        }
        insert(&mut files, name.to_owned(), io(path, fs::read(path))?, path)?;
    }
    for (owner, directory) in &inputs.pictures {
        collect(directory, &format!("pictures/{owner}"), &mut files)?;
    }
    if let Some(directory) = &inputs.files {
        collect(directory, "files", &mut files)?;
    }
    let digest = digest::of(&files);
    if let Some(path) = &inputs.page
        && types::of(path).is_none()
    {
        return Err(Refusal::Type { file: path.clone() });
    }
    let page = page::render(inputs, digest)?;
    if out.exists() {
        io(out, fs::remove_dir_all(out))?;
    }
    let directory = out.join(digest.to_string());
    io(&directory, fs::create_dir_all(&directory))?;
    for (name, bytes) in files {
        let path = directory.join(name);
        let parent = path.parent().expect("a bundled file has a directory");
        io(parent, fs::create_dir_all(parent))?;
        io(&path, fs::write(&path, bytes))?;
    }
    let index = out.join("index.html");
    io(&index, fs::write(&index, page))?;
    Ok(digest)
}

fn collect(directory: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) -> Result<(), Refusal> {
    for entry in io(directory, fs::read_dir(directory))? {
        let entry = io(directory, entry)?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| Refusal::Io { path: path.clone() })?;
        let name = format!("{prefix}/{name}");
        let kind = io(&path, entry.file_type())?;
        if kind.is_dir() {
            collect(&path, &name, files)?;
        } else if kind.is_file() {
            insert(files, name, io(&path, fs::read(&path))?, &path)?;
        } else {
            return Err(Refusal::Io { path });
        }
    }
    Ok(())
}

fn insert(files: &mut BTreeMap<String, Vec<u8>>, name: String, bytes: Vec<u8>, source: &Path) -> Result<(), Refusal> {
    if !Path::new(&name).components().all(|part| matches!(part, std::path::Component::Normal(_))) {
        return Err(Refusal::Io { path: source.to_owned() });
    }
    if types::of(Path::new(&name)).is_none() {
        return Err(Refusal::Type { file: source.to_owned() });
    }
    if files.contains_key(&name) {
        return Err(Refusal::SameName { file: source.to_owned() });
    }
    files.insert(name, bytes);
    Ok(())
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type { file } => write!(f, "unsupported bundle type: {}", file.display()),
            Self::ReservedName { file } => write!(f, "reserved stylesheet name: {}", file.display()),
            Self::SameName { file } => write!(f, "duplicate bundle name: {}", file.display()),
            Self::NoMarker { page } => write!(f, "page has no {{bundle}} marker: {}", page.display()),
            Self::Io { path } => write!(f, "cannot read or write: {}", path.display()),
            Self::Worker => f.write_str("worker name and glue must be given together"),
            Self::Glue => f.write_str("cannot generate module glue"),
        }
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
mod tests;
