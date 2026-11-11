// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::hardware::{BindError, Slot};

static FIRST: Slot<&'static usize> = Slot::new();
static SECOND: Slot<&'static usize> = Slot::new();

arch::declare_hardware!(static FIRST_VALUE: usize = 7 => bind_first);
arch::declare_hardware!(static SECOND_VALUE: usize = 19 => bind_second);

fn bind_first(value: &'static usize) -> Result<(), BindError> {
    FIRST.bind(value)
}

fn bind_second(value: &'static usize) -> Result<(), BindError> {
    SECOND.bind(value)
}

#[test]
fn declarations_are_collected_without_initializing_or_replacing_instances() {
    assert_eq!(FIRST.try_get(), None);
    assert_eq!(SECOND.try_get(), None);
    assert!(std::panic::catch_unwind(kernel::hardware::mmu).is_err());

    arch::hardware::bind();
    let (first, second) = (FIRST.get(), SECOND.get());
    assert_eq!((*first, *second), (7, 19));
    assert!(std::panic::catch_unwind(arch::hardware::bind).is_err());
    assert!(core::ptr::eq(first, FIRST.get()));
    assert!(core::ptr::eq(second, SECOND.get()));
}
