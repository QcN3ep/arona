// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Execute a configured number of work units inside one instruction measurement interval.

/// One performance case's work count and architecture measurement boundaries.
pub struct Perf {
    iterations: u64,
    measured: bool,
    start: extern "C" fn(),
    end: extern "C" fn(),
}

impl Perf {
    /// Configure the case before calling its registered function.
    pub fn new(iterations: u64, start: extern "C" fn(), end: extern "C" fn()) -> Self {
        assert!(iterations > 0, "performance iterations must be positive");
        Self {
            iterations,
            measured: false,
            start,
            end,
        }
    }

    /// Execute `operation` once per work unit. Loop, call, and result destruction costs are included.
    pub fn iter<T>(&mut self, mut operation: impl FnMut() -> T) {
        assert!(
            !self.measured,
            "a performance case must measure exactly once"
        );
        self.measured = true;
        (self.start)();
        for _ in 0..self.iterations {
            core::hint::black_box(operation());
        }
        (self.end)();
    }

    /// Transfer the interval to a runner that executes the case in another context.
    pub fn delegate(&mut self, run: impl FnOnce(Self)) {
        assert!(
            !self.measured,
            "a performance case must measure exactly once"
        );
        run(Self::new(self.iterations, self.start, self.end));
        self.measured = true;
    }

    /// Require a completed measurement before reporting the case as successful.
    pub fn finish(self) {
        assert!(self.measured, "performance case did not measure any work");
    }
}
