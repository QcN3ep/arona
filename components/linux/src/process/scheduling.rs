// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux policy and nice translation into native CPU shares.

use super::Thread;
use runtime::Handle;

pub(crate) const SCHED_OTHER: i32 = 0;
pub(crate) const SCHED_BATCH: i32 = 3;
pub(crate) const SCHED_IDLE: i32 = 5;

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/kernel/sched/core.c?h=v6.5
const WEIGHTS: [u32; 40] = [
    88761, 71755, 56483, 46273, 36291, 29154, 23254, 18705, 14949, 11916, 9548, 7620, 6100, 4904,
    3906, 3121, 2501, 1991, 1586, 1277, 1024, 820, 655, 526, 423, 335, 272, 215, 172, 137, 110, 87,
    70, 56, 45, 36, 29, 23, 18, 15,
];

fn weight(policy: i32, nice: i8) -> u32 {
    if policy == SCHED_IDLE {
        3
    } else {
        WEIGHTS[(i16::from(nice) + 20) as usize]
    }
}

impl Thread<'_> {
    pub(crate) fn inherit_scheduling(
        &self,
        native: &Handle<runtime::Thread>,
    ) -> Result<(i32, i8), runtime::Error> {
        native.set_weight(weight(self.policy, self.nice))?;
        Ok((self.policy, self.nice))
    }

    pub(crate) fn set_scheduling(&mut self, policy: i32, nice: i8) -> Result<(), runtime::Error> {
        self.native.set_weight(weight(policy, nice))?;
        self.policy = policy;
        self.nice = nice;
        Ok(())
    }
}
