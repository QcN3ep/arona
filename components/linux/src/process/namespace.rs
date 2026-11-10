// SPDX-License-Identifier: MPL-2.0

//! Linux 进程与线程的标识空间。

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

pub(crate) struct PidNamespace {
    next: Cell<usize>,
    pub processes: RefCell<BTreeMap<usize, ProcessRecord>>,
    pub futexes: Rc<super::futex::Futexes>,
}

impl PidNamespace {
    pub fn new() -> Self {
        Self {
            next: Cell::new(1),
            processes: RefCell::new(BTreeMap::new()),
            futexes: Rc::default(),
        }
    }

    pub fn allocate(&self) -> Option<usize> {
        let id = self.next.get();
        if id > i32::MAX as usize {
            return None;
        }
        self.next.set(id + 1);
        Some(id)
    }
}

pub(crate) struct ProcessRecord {
    pub parent: usize,
    pub group: usize,
    pub status: Option<crate::ExitStatus>,
}

impl PidNamespace {
    pub fn register(&self, pid: usize, parent: usize, group: usize) {
        self.processes.borrow_mut().insert(
            pid,
            ProcessRecord {
                parent,
                group,
                status: None,
            },
        );
    }

    pub fn parent(&self, pid: usize) -> usize {
        self.processes
            .borrow()
            .get(&pid)
            .map_or(0, |record| record.parent)
    }

    pub fn exit(&self, pid: usize, status: crate::ExitStatus) {
        let mut processes = self.processes.borrow_mut();
        for record in processes.values_mut().filter(|record| record.parent == pid) {
            record.parent = 0;
        }
        if let Some(record) = processes.get_mut(&pid) {
            record.status = Some(status);
        }
        processes.retain(|_, record| record.parent != 0 || record.status.is_none());
    }
}
