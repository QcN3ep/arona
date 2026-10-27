// SPDX-License-Identifier: MPL-2.0

use crate::{
    hardware::cpu::InterruptGuard,
    task::{HandleEntry, Object, Task},
};
use abi::{Error, Handle, Rights};
use arrayvec::ArrayVec;

pub(super) fn create<const N: usize>(
    caller: &Task,
    rights: [Rights; N],
    build: impl FnOnce() -> Result<[Object; N], Error>,
) -> Result<[Handle; N], Error> {
    let _interrupts = InterruptGuard::new();
    let mut reservation = caller.reserve_handles(N)?;
    let handles = core::array::from_fn(|index| reservation.handles[index]);
    let objects = build()?;
    let mut pending: Pending<N> = Pending {
        entries: objects
            .into_iter()
            .zip(rights)
            .map(|(object, rights)| HandleEntry { object, rights })
            .collect(),
    };
    reservation.publish(&mut pending.entries)?;
    Ok(handles)
}

struct Pending<const N: usize> {
    entries: ArrayVec<HandleEntry, N>,
}
impl<const N: usize> Drop for Pending<N> {
    fn drop(&mut self) {
        for entry in &self.entries {
            match &entry.object {
                Object::Task(task) => task.terminate(0),
                Object::Thread(thread) => {
                    let _ = thread.terminate(0);
                }
                _ => {}
            }
        }
    }
}
