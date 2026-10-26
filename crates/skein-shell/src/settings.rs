//! A command's settings, their sources, help and refusals (shell.md, 6.5).

use std::fmt;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

type EnvironmentEntry = (Box<[u8]>, Box<[u8]>);

/// A command's settings (shell.md, 6.5): its tool's name and its table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// The tool's name, lowercase ASCII letters, digits and `-`: `temper`, `skein-echo`.
    pub tool: &'static str,
    pub table: &'static [Setting],
}

/// One setting: the flag `--name VALUE`, the variable `<TOOL>_<NAME>`
/// (capitals, `-` as `_`), and its default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setting {
    /// The flag's name without its dashes, lowercase ASCII letters, digits and `-`: `config`.
    pub name: &'static str,
    /// What the value is, for help and refusals: `DIR`, `BYTES`.
    pub value: &'static str,
    /// One line for help.
    pub about: &'static str,
    pub fallback: Fallback,
}

/// What a setting is when neither its flag nor its variable gives it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    /// No sensible default: the start is refused.
    Refuse,
    /// A fixed value, documented as given.
    Text(&'static str),
    /// An XDG base directory, then the tool's name: `$XDG_CONFIG_HOME/temper`.
    Base(Base),
}

/// XDG's base directories, each with the specification's fallback under `$HOME`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    /// `XDG_CONFIG_HOME`, else `$HOME/.config`.
    Config,
    /// `XDG_DATA_HOME`, else `$HOME/.local/share`.
    Data,
    /// `XDG_STATE_HOME`, else `$HOME/.local/state`.
    State,
    /// `XDG_CACHE_HOME`, else `$HOME/.cache`.
    Cache,
}

impl Base {
    fn variable(self) -> &'static str {
        match self {
            Self::Config => "XDG_CONFIG_HOME",
            Self::Data => "XDG_DATA_HOME",
            Self::State => "XDG_STATE_HOME",
            Self::Cache => "XDG_CACHE_HOME",
        }
    }

    fn suffix(self) -> &'static [u8] {
        match self {
            Self::Config => b".config",
            Self::Data => b".local/share",
            Self::State => b".local/state",
            Self::Cache => b".cache",
        }
    }
}

/// Where a setting's value came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Flag,
    Variable,
    /// The setting's `Fallback::Text`.
    Default,
    /// `Fallback::Base` through its XDG variable.
    Base,
    /// `Fallback::Base` through the specification's fallback under `$HOME`.
    Home,
}

/// A setting's bytes and the source that supplied them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Value {
    pub bytes: Box<[u8]>,
    pub origin: Origin,
}

/// Every setting's value, in the table's order, and the arguments that
/// were no setting's, in their order: the binary's own.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Resolved {
    values: Box<[Value]>,
    rest: Box<[Box<[u8]>]>,
}

impl Resolved {
    /// The value of the table's row `row`. Panics when `row` is not a
    /// row of the table: the binary's own mistake, which its tests catch.
    #[must_use]
    pub fn value(&self, row: usize) -> &Value {
        self.values.get(row).expect("row belongs to the settings table")
    }

    /// The binary's own arguments, in their given order.
    #[must_use]
    pub fn rest(&self) -> &[Box<[u8]>] {
        &self.rest
    }
}

/// The setting a start refused, and why.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refusal {
    pub tool: &'static str,
    pub setting: Setting,
    pub reason: Reason,
}

/// Why a setting refused the start.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// Neither the flag nor the variable gives it, and it has no default.
    Missing,
    /// The flag is the last argument, without its value.
    NoValue,
    /// The flag is given twice.
    Twice,
    /// Its default's base: neither the XDG variable nor `HOME` is an absolute path.
    NoBase(Base),
}

impl fmt::Display for Refusal {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Setting { name, value, .. } = self.setting;
        let variable = variable(self.tool, name);
        match self.reason {
            Reason::Missing => write!(out, "--{name} {value} is not given, and {variable} is not set"),
            Reason::NoValue => write!(out, "--{name} takes a value: {value}"),
            Reason::Twice => write!(out, "--{name} is given twice"),
            Reason::NoBase(base) => write!(
                out,
                "--{name} {value} is not given, {variable} is not set, and its default {} needs {} or HOME as an absolute path",
                base_default(self.tool, base),
                base.variable(),
            ),
        }
    }
}

/// Resolves a command's settings (shell.md, 6.5) from the arguments after
/// the binary's own command words, and the process's environment.
#[expect(clippy::type_complexity, reason = "shell.md, 6.5 takes byte environment pairs without a wrapper type")]
pub fn resolve(
    settings: &Settings,
    arguments: &[Box<[u8]>],
    environment: &[(Box<[u8]>, Box<[u8]>)],
) -> Result<Resolved, Refusal> {
    let mut flags = vec![None; settings.table.len()];
    let mut rest = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument.as_ref() == b"--" {
            rest.push(argument.clone());
            rest.extend(arguments.cloned());
            break;
        }
        let mut taken = false;
        for (setting, flag) in settings.table.iter().zip(&mut flags) {
            if argument.strip_prefix(b"--") != Some(setting.name.as_bytes()) {
                continue;
            }
            let refusal = |reason| Refusal { tool: settings.tool, setting: *setting, reason };
            if flag.is_some() {
                return Err(refusal(Reason::Twice));
            }
            let value = arguments.next().ok_or_else(|| refusal(Reason::NoValue))?;
            *flag = Some(Value { bytes: value.clone(), origin: Origin::Flag });
            taken = true;
            break;
        }
        if !taken {
            rest.push(argument.clone());
        }
    }
    let mut values = Vec::new();
    for (setting, flag) in settings.table.iter().zip(flags) {
        if let Some(value) = flag {
            values.push(value);
            continue;
        }
        let value = if let Some(bytes) = lookup(environment, variable(settings.tool, setting.name).as_bytes()) {
            Value { bytes: Box::from(bytes), origin: Origin::Variable }
        } else {
            fallback(settings.tool, setting.fallback, environment).map_err(|reason| Refusal {
                tool: settings.tool,
                setting: *setting,
                reason,
            })?
        };
        values.push(value);
    }
    Ok(Resolved { values: values.into_boxed_slice(), rest: rest.into_boxed_slice() })
}

fn fallback(tool: &str, fallback: Fallback, environment: &[EnvironmentEntry]) -> Result<Value, Reason> {
    match fallback {
        Fallback::Refuse => Err(Reason::Missing),
        Fallback::Text(text) => Ok(Value { bytes: Box::from(text.as_bytes()), origin: Origin::Default }),
        Fallback::Base(base) => {
            let (mut path, origin) = if let Some(path) =
                lookup(environment, base.variable().as_bytes()).filter(|path| path.starts_with(b"/"))
            {
                (path.to_vec(), Origin::Base)
            } else if let Some(home) = lookup(environment, b"HOME").filter(|path| path.starts_with(b"/")) {
                let mut path = home.to_vec();
                join(&mut path, base.suffix());
                (path, Origin::Home)
            } else {
                return Err(Reason::NoBase(base));
            };
            join(&mut path, tool.as_bytes());
            Ok(Value { bytes: path.into_boxed_slice(), origin })
        }
    }
}

fn join(path: &mut Vec<u8>, component: &[u8]) {
    if !path.ends_with(b"/") {
        path.push(b'/');
    }
    path.extend_from_slice(component);
}

fn lookup<'a>(environment: &'a [EnvironmentEntry], name: &[u8]) -> Option<&'a [u8]> {
    for (key, value) in environment {
        if key.as_ref() == name {
            return if value.is_empty() { None } else { Some(value) };
        }
    }
    None
}

fn variable(tool: &str, name: &str) -> String {
    format!("{tool}_{name}").replace('-', "_").to_ascii_uppercase()
}

fn base_default(tool: &str, base: Base) -> String {
    format!("${}/{tool}", base.variable())
}

/// Appends one entry per setting for a binary's `--help`.
pub fn help(settings: &Settings, out: &mut Vec<u8>) {
    for setting in settings.table {
        let default = match setting.fallback {
            Fallback::Refuse => String::from("required"),
            Fallback::Text(text) => format!("default {text}"),
            Fallback::Base(base) => format!("default {}", base_default(settings.tool, base)),
        };
        out.extend_from_slice(
            format!(
                "  --{} {} ({}; {default})\n      {}\n",
                setting.name,
                setting.value,
                variable(settings.tool, setting.name),
                setting.about,
            )
            .as_bytes(),
        );
    }
}

/// The process's arguments after its program name, as bytes.
#[must_use]
#[cfg(unix)]
pub fn arguments() -> Box<[Box<[u8]>]> {
    std::env::args_os().skip(1).map(|argument| Box::from(argument.as_bytes())).collect()
}

/// The process's environment, as bytes, in the kernel's order.
#[must_use]
#[expect(clippy::type_complexity, reason = "shell.md, 6.5 returns byte environment pairs without a wrapper type")]
#[cfg(unix)]
pub fn environment() -> Box<[(Box<[u8]>, Box<[u8]>)]> {
    std::env::vars_os().map(|(key, value)| (Box::from(key.as_bytes()), Box::from(value.as_bytes()))).collect()
}

#[cfg(test)]
mod tests;
