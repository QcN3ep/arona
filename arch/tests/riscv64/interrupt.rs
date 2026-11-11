// SPDX-License-Identifier: MPL-2.0

use arch::boot::dtb::Dtb;
use kernel::memory::PhysRange;

pub fn boot(cpu_id: usize, dtb: usize) -> (u64, Dtb<'static>, Option<PhysRange>) {
    let pointer = (0xffff_ffc0_0040_0000 + (dtb & 0x1f_ffff)) as *const u8;
    // SAFETY: The firmware DTB must be fully mapped into the boot firmware window and remain unchanged.
    let tree = unsafe { Dtb::new(pointer) }.unwrap();
    let program = tree.initramfs().unwrap();
    (cpu_id as u64, tree, program)
}
