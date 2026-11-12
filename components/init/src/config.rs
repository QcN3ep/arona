// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Boot program configuration.

// TODO: Configure file, device, and service access authorization for applications.

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub program: Vec<Program>,
    #[serde(default)]
    pub power: Power,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Power {
    #[serde(default)]
    pub action: Option<PowerAction>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerAction {
    Shutdown,
    Reboot,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub name: String,
    pub executor: Executor,
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub terminal: Option<Terminal>,
    #[serde(default)]
    pub restart: Restart,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Executor {
    Linux,
    // TODO: Integrate the native program execution service.
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Terminal {
    Console,
    // TODO: Support specifying terminal devices and pseudo-terminals.
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    #[default]
    Never,
    OnFailure,
    Always,
}

#[derive(Debug)]
pub enum Error {
    Syntax(toml::de::Error),
    InvalidProgram(String),
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let config: Self = toml::from_str(text).map_err(Error::Syntax)?;
        let mut names = BTreeSet::new();
        for program in &config.program {
            if program.name.is_empty()
                || !names.insert(&program.name)
                || !program.path.starts_with('/')
                || program.path.as_bytes().contains(&0)
                || program
                    .args
                    .iter()
                    .any(|argument| argument.as_bytes().contains(&0))
                || program.env.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.contains('=')
                        || key.as_bytes().contains(&0)
                        || value.as_bytes().contains(&0)
                })
            {
                return Err(Error::InvalidProgram(program.name.clone()));
            }
        }
        Ok(config)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Syntax(error) => write!(output, "{error}"),
            Self::InvalidProgram(name) => write!(output, "invalid program: {name}"),
        }
    }
}
