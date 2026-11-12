// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Asynchronous reads of executable files and startup entropy.

// TODO: Parse script interpreters and construct interpreter startup arguments.

use crate::{
    Error, Services,
    elf::{Image, Machine, PAGE_SIZE},
};
use alloc::vec::Vec;
use fs::ReadAt;
use runtime::{Call, Handle};

pub(crate) struct Executable<'a> {
    stage: Stage<'a>,
    random: bool,
}

enum Stage<'a> {
    Open(fs::client::PendingNode<'a>),
    Check {
        file: Option<fs::File<'a>>,
        pending: fs::client::PendingMetadata,
    },
    Read(Reading<'a>),
    Entropy {
        bytes: Vec<u8>,
        request: devmgr::rng::PendingRead,
        random: Vec<u8>,
    },
    Ready(Loaded),
}

pub(crate) struct Loaded {
    pub bytes: Vec<u8>,
    pub random: Option<[u8; 16]>,
}

pub(crate) struct Images {
    pub main: Loaded,
    pub interpreter: Option<Loaded>,
}

/// Reads an executable and its PT_INTERP interpreter in sequence.
pub(crate) struct Chain<'a> {
    executable: Executable<'a>,
    main: Option<Loaded>,
    interpreter: Option<Executable<'a>>,
}

struct Reading<'a> {
    file: fs::File<'a>,
    pending: fs::client::PendingRead,
    bytes: Vec<u8>,
}

impl<'a> Chain<'a> {
    pub fn new(executable: Executable<'a>) -> Self {
        Self {
            executable,
            main: None,
            interpreter: None,
        }
    }

    pub fn poll(
        &mut self,
        files: &fs::Client<'a>,
        machine: Machine,
        services: &Services<'_>,
    ) -> Result<Option<Images>, Error> {
        if self.main.is_none() {
            let Some(loaded) = self.executable.poll(services)? else {
                return Ok(None);
            };
            let image = Image::parse(&loaded.bytes, machine)?;
            if let Some(path) = image.interpreter {
                self.interpreter = Some(Executable::interpreter(files, path)?);
            }
            self.main = Some(loaded);
        }
        let interpreter = match &mut self.interpreter {
            Some(executable) => match executable.poll(services)? {
                Some(loaded) => Some(loaded),
                None => return Ok(None),
            },
            None => None,
        };
        Ok(Some(Images {
            main: self.main.take().ok_or(Error::InvalidExecutable)?,
            interpreter,
        }))
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        self.interpreter
            .as_ref()
            .and_then(Executable::call)
            .or_else(|| self.executable.call())
    }
}

impl<'a> Executable<'a> {
    pub fn new(
        files: &fs::Client<'a>,
        directory: Option<&fs::Directory<'a>>,
        path: &[u8],
    ) -> Result<Self, Error> {
        Self::open(files, directory, path, true)
    }

    pub fn interpreter(files: &fs::Client<'a>, path: &[u8]) -> Result<Self, Error> {
        Self::open(files, None, path, false)
    }

    fn open(
        files: &fs::Client<'a>,
        directory: Option<&fs::Directory<'a>>,
        path: &[u8],
        random: bool,
    ) -> Result<Self, Error> {
        let request = files.start_open_at(
            directory,
            path,
            fs::OpenOptions {
                write: false,
                create: false,
                exclusive: false,
                truncate: false,
                permissions: 0,
            },
            true,
            false,
        )?;
        Ok(Self {
            stage: Stage::Open(request),
            random,
        })
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.stage {
            Stage::Open(request) => Some(request.call()),
            Stage::Check { pending, .. } => Some(pending.call()),
            Stage::Read(read) => Some(read.pending.call()),
            Stage::Entropy { request, .. } => Some(request.call()),
            Stage::Ready(_) => None,
        }
    }

    pub fn poll(&mut self, services: &Services<'_>) -> Result<Option<Loaded>, Error> {
        let random = self.random;
        match &mut self.stage {
            Stage::Open(request) => {
                if let Some(node) = completed(request.try_complete())? {
                    let fs::client::OpenedNode::File(file) = node else {
                        return Err(Error::PermissionDenied);
                    };
                    let pending = file.start_metadata()?;
                    self.stage = Stage::Check {
                        file: Some(file),
                        pending,
                    };
                }
            }
            Stage::Check { file, pending } => {
                if let Some(metadata) = completed(pending.try_complete())? {
                    if metadata.mode & 0o111 == 0 {
                        return Err(Error::PermissionDenied);
                    }
                    self.stage = Stage::Read(Reading::new(file.take().unwrap())?);
                }
            }
            Stage::Read(read) => {
                if read.poll()? {
                    let bytes = core::mem::take(&mut read.bytes);
                    self.stage = if random && let Some(source) = &services.entropy {
                        Stage::Entropy {
                            bytes,
                            request: source.start_read(16)?,
                            random: Vec::new(),
                        }
                    } else {
                        Stage::Ready(Loaded {
                            bytes,
                            random: None,
                        })
                    };
                }
            }
            Stage::Entropy {
                bytes,
                request,
                random,
            } => match request.try_complete() {
                Ok(data) => {
                    random.extend_from_slice(&data);
                    if random.len() == 16 {
                        self.stage = Stage::Ready(Loaded {
                            bytes: core::mem::take(bytes),
                            random: Some(random.as_slice().try_into().unwrap()),
                        });
                    } else {
                        *request = services
                            .entropy
                            .as_ref()
                            .unwrap()
                            .start_read(16 - random.len())?;
                    }
                }
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error.into()),
            },
            Stage::Ready(_) => {}
        }
        if matches!(self.stage, Stage::Ready(_)) {
            let Stage::Ready(loaded) = core::mem::replace(
                &mut self.stage,
                Stage::Ready(Loaded {
                    bytes: Vec::new(),
                    random: None,
                }),
            ) else {
                unreachable!()
            };
            return Ok(Some(loaded));
        }
        Ok(None)
    }
}

fn completed<T>(result: Result<T, fs::Error>) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

impl<'a> Reading<'a> {
    fn new(file: fs::File<'a>) -> Result<Self, Error> {
        if file.is_empty() {
            return Err(Error::InvalidExecutable);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(file.len())
            .map_err(|_| Error::NoMemory)?;
        let pending = file.start_read_at(0, file.len().min(PAGE_SIZE))?;
        Ok(Self {
            file,
            bytes,
            pending,
        })
    }

    fn poll(&mut self) -> Result<bool, Error> {
        let Some(chunk) = completed(self.pending.try_complete())? else {
            return Ok(false);
        };
        if chunk.is_empty() {
            return Err(Error::InvalidExecutable);
        }
        self.bytes.extend_from_slice(&chunk);
        if self.bytes.len() == self.file.len() {
            return Ok(true);
        }
        self.pending = self.file.start_read_at(
            self.bytes.len(),
            (self.file.len() - self.bytes.len()).min(PAGE_SIZE),
        )?;
        Ok(false)
    }
}
