// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Provide a peer that blocks between requests on the single-CPU scheduler.

use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    ipc::{Channel, EventReference},
    task::{Handle, Task},
    thread::{self, Thread, ThreadState},
};
use spin::Mutex;

pub(super) enum Job {
    Call(Task, Handle),
    Signal(Channel, EventReference),
    Yield,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);

pub(super) fn start(job: Job) -> Thread {
    let yielding = matches!(&job, Job::Yield);
    STOP.store(false, Ordering::Relaxed);
    assert!(JOB.lock().replace(job).is_none());
    let worker = Thread::new(run, 0).unwrap();
    worker.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(
        worker.state(),
        if yielding {
            ThreadState::Ready
        } else {
            ThreadState::Blocked
        }
    );
    worker
}

pub(super) fn stop_yield() {
    STOP.store(true, Ordering::Relaxed);
}

fn run(_: usize) -> usize {
    let job = JOB.lock().take().unwrap();
    let mut rounds = 0;
    match job {
        Job::Call(task, channel) => loop {
            let request = task.receive(channel, 1).unwrap();
            if request.message.tag == 0 {
                break;
            }
            let reply = request.handles()[0];
            task.reply(reply, request.message, &[]).unwrap();
            task.close(reply).unwrap();
            rounds += 1;
        },
        Job::Signal(channel, event) => loop {
            if channel.receive().unwrap().tag == 0 {
                break;
            }
            event.signal(1).unwrap();
            rounds += 1;
        },
        Job::Yield => {
            while !STOP.load(Ordering::Relaxed) {
                rounds += 1;
                thread::yield_now().unwrap();
            }
        }
    }
    rounds
}
