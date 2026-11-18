// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File descriptors and shared open state.

pub(crate) mod block;
pub(crate) mod pipe;
pub(crate) mod socket;
pub(crate) mod socketpair;
pub(crate) mod terminal;

use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use runtime::{Event, Handle, Timer};

use crate::process::mqueue::Queue;

pub(crate) const O_APPEND: u32 = 0x400;
pub(crate) const O_NONBLOCK: u32 = 0x800;
pub(crate) const O_DSYNC: u32 = 0x1000;
pub(crate) const O_PATH: u32 = 0x200000;

pub(crate) struct FileTable<'a> {
    entries: BTreeMap<usize, Descriptor<'a>>,
    maximum: usize,
}

pub(crate) struct Descriptor<'a> {
    /// Identity retained when a Linux-local node opens a device or snapshot.
    pub node: Option<fs::Metadata>,
    pub description: Rc<OpenFileDescription<'a>>,
    pub close_on_exec: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Virtual {
    Null,
    Zero,
    Random,
}

pub(crate) enum OpenFileDescription<'a> {
    Socket {
        state: socket::Socket<'a>,
        flags: Cell<u32>,
    },
    SocketPair {
        state: socketpair::SocketPair,
        flags: Cell<u32>,
    },
    Pipe {
        end: pipe::PipeEnd,
        flags: Cell<u32>,
    },
    Terminal {
        terminal: Rc<terminal::Terminal>,
        flags: Cell<u32>,
    },
    Directory {
        session: fs::Directory<'a>,
        flags: Cell<u32>,
        offset: Cell<usize>,
        busy: Cell<bool>,
        entries: RefCell<Option<Vec<crate::vfs::Entry>>>,
    },
    Regular {
        file: fs::File<'a>,
        flags: Cell<u32>,
        offset: Cell<usize>,
        busy: Cell<bool>,
        /// Name used to open the file, kept as the backing path for mappings.
        path: Option<Rc<Vec<u8>>>,
    },
    Block {
        device: Rc<block::BlockDevice>,
        flags: Cell<u32>,
        offset: Cell<usize>,
    },
    Signalfd {
        mask: Cell<u64>,
        flags: Cell<u32>,
    },
    Timerfd {
        state: TimerState,
        flags: Cell<u32>,
    },
    Eventfd {
        counter: Cell<u64>,
        semaphore: bool,
        flags: Cell<u32>,
    },
    Virtual {
        kind: Virtual,
        flags: Cell<u32>,
    },
    Rtc {
        flags: Cell<u32>,
    },
    VirtualFile {
        data: Vec<u8>,
        metadata: fs::Metadata,
        flags: Cell<u32>,
        offset: Cell<usize>,
    },
    VirtualDirectory {
        inode: u64,
        path: Vec<u8>,
        entries: RefCell<Option<Vec<crate::vfs::Entry>>>,
        flags: Cell<u32>,
        offset: Cell<usize>,
        busy: Cell<bool>,
    },
    MessageQueue {
        queue: Rc<Queue>,
        flags: Cell<u32>,
    },
    MqueueFile {
        queue: Rc<Queue>,
        flags: Cell<u32>,
        offset: Cell<usize>,
    },
}

pub(crate) struct TimerState {
    pub next: Cell<Option<u64>>,
    pub interval: Cell<u64>,
    pub event: Handle<Event>,
    pub timer: Handle<Timer>,
}

pub(crate) struct FilePosition<'a> {
    description: Rc<OpenFileDescription<'a>>,
}

impl<'a> FileTable<'a> {
    pub fn new(terminal: Rc<terminal::Terminal>) -> Self {
        let description = Rc::new(OpenFileDescription::Terminal {
            terminal: Rc::clone(&terminal),
            flags: Cell::new(1),
        });
        let mut table = Self {
            maximum: crate::limits::Limits::default().file_descriptors(),
            entries: [
                (
                    1,
                    Descriptor {
                        node: None,
                        description: Rc::clone(&description),
                        close_on_exec: false,
                    },
                ),
                (
                    2,
                    Descriptor {
                        node: None,
                        description,
                        close_on_exec: false,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };
        if terminal.has_input() {
            table.entries.insert(
                0,
                Descriptor {
                    node: None,
                    description: Rc::new(OpenFileDescription::Terminal {
                        terminal,
                        flags: Cell::new(0),
                    }),
                    close_on_exec: false,
                },
            );
        }
        table
    }

    pub fn get(&self, fd: usize) -> Option<&Descriptor<'a>> {
        self.entries.get(&fd)
    }

    pub fn get_mut(&mut self, fd: usize) -> Option<&mut Descriptor<'a>> {
        self.entries.get_mut(&fd)
    }

    pub fn insert(&mut self, descriptor: Descriptor<'a>, minimum: usize) -> Option<usize> {
        let mut available = minimum;
        for &fd in self.entries.range(minimum..).map(|(fd, _)| fd) {
            if fd != available {
                break;
            }
            available += 1;
        }
        if available >= self.maximum {
            return None;
        }
        self.entries.insert(available, descriptor);
        Some(available)
    }

    pub fn replace(&mut self, fd: usize, descriptor: Descriptor<'a>) {
        self.entries.insert(fd, descriptor);
    }

    pub fn close(&mut self, fd: usize) -> bool {
        self.entries.remove(&fd).is_some()
    }
}

impl<'a> Descriptor<'a> {
    pub fn terminal(terminal: Rc<terminal::Terminal>, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Terminal {
                terminal,
                flags: Cell::new(flags),
            }),
            close_on_exec,
        }
    }
    pub fn signalfd(mask: u64, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Signalfd {
                mask: Cell::new(mask),
                flags: Cell::new(2 | flags),
            }),
            close_on_exec,
        }
    }

    pub fn timerfd(state: TimerState, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Timerfd {
                state,
                flags: Cell::new(2 | flags),
            }),
            close_on_exec,
        }
    }

    pub fn eventfd(initial: u32, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Eventfd {
                counter: Cell::new(u64::from(initial)),
                semaphore: flags & 1 != 0,
                flags: Cell::new(2 | (flags & O_NONBLOCK)),
            }),
            close_on_exec,
        }
    }
    pub fn rtc(flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Rtc {
                flags: Cell::new(flags),
            }),
            close_on_exec,
        }
    }

    pub fn virtual_file(kind: Virtual, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Virtual {
                kind,
                flags: Cell::new(flags),
            }),
            close_on_exec,
        }
    }
    pub fn block(device: Rc<block::BlockDevice>, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Block {
                device,
                flags: Cell::new(flags),
                offset: Cell::new(0),
            }),
            close_on_exec,
        }
    }

    pub fn regular(
        file: fs::File<'a>,
        flags: u32,
        close_on_exec: bool,
        path: Option<Rc<Vec<u8>>>,
    ) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::Regular {
                file,
                flags: Cell::new(flags),
                offset: Cell::new(0),
                busy: Cell::new(false),
                path,
            }),
            close_on_exec,
        }
    }

    pub fn snapshot(data: Vec<u8>, inode: u64, flags: u32, close_on_exec: bool) -> Self {
        let metadata = fs::Metadata {
            device: 0x7072,
            inode,
            size: data.len() as u64,
            blocks: (data.len() as u64).div_ceil(512),
            links: 1,
            mode: 0o100444,
            ..fs::Metadata::default()
        };
        Self::snapshot_node(data, metadata, flags, close_on_exec)
    }

    pub fn snapshot_node(
        data: Vec<u8>,
        metadata: fs::Metadata,
        flags: u32,
        close_on_exec: bool,
    ) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::VirtualFile {
                data,
                metadata,
                flags: Cell::new(flags),
                offset: Cell::new(0),
            }),
            close_on_exec,
        }
    }

    pub fn virtual_directory(inode: u64, flags: u32, close_on_exec: bool) -> Self {
        Self::local_directory(b"/dev/mqueue".to_vec(), inode, flags, close_on_exec)
    }

    pub fn local_directory(path: Vec<u8>, inode: u64, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::VirtualDirectory {
                inode,
                path,
                entries: RefCell::new(None),
                flags: Cell::new(flags),
                offset: Cell::new(0),
                busy: Cell::new(false),
            }),
            close_on_exec,
        }
    }

    pub fn message_queue(queue: Rc<Queue>, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::MessageQueue {
                queue,
                flags: Cell::new(flags),
            }),
            close_on_exec,
        }
    }

    pub fn mqueue_file(queue: Rc<Queue>, flags: u32, close_on_exec: bool) -> Self {
        Self {
            node: None,
            description: Rc::new(OpenFileDescription::MqueueFile {
                queue,
                flags: Cell::new(flags & 3),
                offset: Cell::new(0),
            }),
            close_on_exec,
        }
    }

    pub fn duplicate(&self, close_on_exec: bool) -> Self {
        Self {
            node: self.node,
            description: Rc::clone(&self.description),
            close_on_exec,
        }
    }
}

impl<'a> OpenFileDescription<'a> {
    pub fn flags(&self) -> u32 {
        match self {
            Self::Socket { flags, .. }
            | Self::SocketPair { flags, .. }
            | Self::Pipe { flags, .. }
            | Self::Terminal { flags, .. }
            | Self::Directory { flags, .. }
            | Self::Regular { flags, .. }
            | Self::Block { flags, .. }
            | Self::Eventfd { flags, .. }
            | Self::Timerfd { flags, .. }
            | Self::Signalfd { flags, .. }
            | Self::Virtual { flags, .. }
            | Self::VirtualFile { flags, .. }
            | Self::Rtc { flags }
            | Self::VirtualDirectory { flags, .. }
            | Self::MessageQueue { flags, .. }
            | Self::MqueueFile { flags, .. } => flags.get(),
        }
    }

    pub fn set_flags(&self, value: u32) {
        let (Self::Socket { flags, .. }
        | Self::SocketPair { flags, .. }
        | Self::Pipe { flags, .. }
        | Self::Terminal { flags, .. }
        | Self::Directory { flags, .. }
        | Self::Regular { flags, .. }
        | Self::Block { flags, .. }
        | Self::Eventfd { flags, .. }
        | Self::Timerfd { flags, .. }
        | Self::Signalfd { flags, .. }
        | Self::Virtual { flags, .. }
        | Self::VirtualFile { flags, .. }
        | Self::Rtc { flags }
        | Self::VirtualDirectory { flags, .. }
        | Self::MessageQueue { flags, .. }
        | Self::MqueueFile { flags, .. }) = self;
        flags.set((flags.get() & !(O_APPEND | O_NONBLOCK)) | (value & (O_APPEND | O_NONBLOCK)));
    }

    /// Whether the description carries only path identity: it answers status queries and
    /// anchors relative operations but permits no I/O, positioning, or directory reads.
    pub fn path_only(&self) -> bool {
        self.flags() & O_PATH != 0
    }

    pub fn position(self: &Rc<Self>) -> Option<FilePosition<'a>> {
        let busy = match self.as_ref() {
            Self::Regular { busy, .. }
            | Self::Directory { busy, .. }
            | Self::VirtualDirectory { busy, .. } => busy,
            _ => return None,
        };
        if busy.replace(true) {
            return None;
        }
        Some(FilePosition {
            description: Rc::clone(self),
        })
    }
}

impl FilePosition<'_> {
    pub fn get(&self) -> usize {
        let offset = match self.description.as_ref() {
            OpenFileDescription::Regular { offset, .. }
            | OpenFileDescription::Directory { offset, .. }
            | OpenFileDescription::VirtualDirectory { offset, .. } => offset,
            _ => unreachable!(),
        };
        offset.get()
    }

    pub fn set(&self, position: usize) {
        let offset = match self.description.as_ref() {
            OpenFileDescription::Regular { offset, .. }
            | OpenFileDescription::Directory { offset, .. }
            | OpenFileDescription::VirtualDirectory { offset, .. } => offset,
            _ => unreachable!(),
        };
        offset.set(position);
        if position == 0
            && let OpenFileDescription::Directory { entries, .. }
            | OpenFileDescription::VirtualDirectory { entries, .. } = self.description.as_ref()
        {
            *entries.borrow_mut() = None;
        }
    }
}

impl Drop for FilePosition<'_> {
    fn drop(&mut self) {
        let busy = match self.description.as_ref() {
            OpenFileDescription::Regular { busy, .. }
            | OpenFileDescription::Directory { busy, .. }
            | OpenFileDescription::VirtualDirectory { busy, .. } => busy,
            _ => unreachable!(),
        };
        busy.set(false);
    }
}

impl FileTable<'_> {
    pub fn fork(&self) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .map(|(&fd, entry)| (fd, entry.duplicate(entry.close_on_exec)))
                .collect(),
            maximum: self.maximum,
        }
    }

    /// Applies the `RLIMIT_NOFILE` soft limit to future descriptor allocations.
    pub fn set_maximum(&mut self, maximum: usize) {
        self.maximum = maximum;
    }
}

impl FileTable<'_> {
    pub fn close_on_exec(&mut self) {
        self.entries
            .retain(|_, descriptor| !descriptor.close_on_exec);
    }
}
