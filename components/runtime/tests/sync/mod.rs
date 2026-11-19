// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod mailbox;
mod mutex;
mod semaphore;

use alloc::sync::Arc;
use runtime::{
    Error,
    ipc::WaitMode,
    sync::Semaphore,
    thread::{self, JoinHandle},
};

pub fn run() {
    semaphore::run();
    mutex::run();
    mailbox::run();
}

fn spawn<T: Send + 'static>(
    function: impl FnOnce() -> T + Send + 'static,
) -> Result<JoinHandle<T>, Error> {
    let entered = Arc::new(Semaphore::new(0));
    let ready = entered.clone();
    let worker = thread::spawn(move || {
        ready.post().unwrap();
        function()
    })?;
    entered.wait(WaitMode::Forever)?;
    Ok(worker)
}
