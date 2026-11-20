// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Execute performance cases on kernel threads.

use spin::Mutex;
pub use testkit_core::perf::Perf;

type ThreadCase = (Perf, fn(&mut Perf));
static THREAD_CASE: Mutex<Option<ThreadCase>> = Mutex::new(None);

/// Run a case on a kernel thread, excluding its creation and exit from the interval.
pub fn in_thread(perf: &mut Perf, operation: fn(&mut Perf)) {
    perf.delegate(|case| {
        assert!(THREAD_CASE.lock().replace((case, operation)).is_none());
        crate::run_thread(|_| {
            let (mut perf, operation) = THREAD_CASE.lock().take().unwrap();
            operation(&mut perf);
            perf.finish();
            0
        });
    });
}
