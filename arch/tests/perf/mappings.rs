// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Exercises real mappings with two programs sharing a virtual address and a reused load instance.

use super::isa;

pub const CODE: usize = 0x1_0000;
pub const DESCRIPTOR: usize = CODE + 4096;
const MAGIC: usize = 0x4152_4f4e_414d_4150;

#[repr(C, align(4096))]
pub struct Page(pub [usize; 512]);

#[repr(C)]
pub struct Space {
    pub tables: [Page; 4],
    pub descriptor: Page,
    pub code: Page,
}

impl Space {
    const fn new() -> Self {
        Self {
            tables: [const { Page([0; 512]) }; 4],
            descriptor: Page([0; 512]),
            code: Page([0; 512]),
        }
    }

    fn initialize(&mut self, instance: usize, template: usize) {
        isa::map_space(self);
        let root = isa::root(self);
        self.descriptor.0[..6].copy_from_slice(&[MAGIC, instance, root, CODE, template, 12]);
        if template != 0 {
            // SAFETY: Both templates contain 12 readable bytes; the inactive code page is exclusively writable.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    template as *const u8,
                    self.code.0.as_mut_ptr().cast(),
                    12,
                )
            };
        }
    }
}

static mut SPACES: [Space; 3] = [const { Space::new() }; 3];

unsafe extern "C" {
    pub static _boot_root: Page;
    pub fn __arona_perf_image_a();
    pub fn __arona_perf_image_b();
}

pub fn prepare() {
    // SAFETY: This single-CPU calibration exclusively owns all inactive space storage.
    let spaces = unsafe { &mut *core::ptr::addr_of_mut!(SPACES) };
    spaces[0].initialize(0, 0);
    spaces[0].descriptor.0[3..6].fill(0);
    spaces[1].initialize(1, __arona_perf_image_a as *const () as usize);
    spaces[2].initialize(2, __arona_perf_image_b as *const () as usize);
    isa::switch_space(isa::root(&spaces[0]));
}

#[inline(never)]
pub extern "C" fn run() -> usize {
    // SAFETY: This single-CPU scenario owns the space storage; all roots share the current kernel mapping.
    let spaces = unsafe { &mut *core::ptr::addr_of_mut!(SPACES) };
    let baseline = isa::root(&spaces[0]);
    let first = isa::root(&spaces[1]);
    let second = isa::root(&spaces[2]);
    let mut result = 0;
    for root in [first, second, first] {
        isa::switch_space(root);
        result += call();
    }
    isa::switch_space(baseline);
    spaces[1].tables.iter_mut().for_each(|page| page.0.fill(0));
    spaces[1].initialize(3, __arona_perf_image_b as *const () as usize);
    assert_eq!(isa::root(&spaces[1]), first);
    for root in [first, second] {
        isa::switch_space(root);
        result += call();
    }
    isa::switch_space(baseline);
    // Removing the descriptor leaves executable code whose lifetime the collector cannot identify.
    isa::hide_descriptor(&mut spaces[2]);
    isa::switch_space(second);
    result += call();
    isa::switch_space(baseline);
    result
}

fn call() -> usize {
    // SAFETY: The active space maps an executable template at CODE with the C ABI and a valid return instruction.
    let entry: extern "C" fn() -> usize = unsafe { core::mem::transmute(CODE) };
    entry()
}
