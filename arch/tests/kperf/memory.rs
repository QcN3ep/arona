// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register memory performance cases with the kernel's normal direct mapping active.

use super::setup::initialize_memory as initialize;
use kernel::{
    hardware::mmu::PagePermissions,
    memory::{FrameAccess, FrameOwner},
};
use testkit::{memory as cases, perf::Perf};

mod frame_allocator {
    use super::*;

    #[testkit::kperf]
    fn kernel_1_page(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::Kernel, 1);
    }

    #[testkit::kperf]
    fn kernel_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::Kernel, 16);
    }

    #[testkit::kperf]
    fn kernel_256_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::Kernel, 256);
    }

    #[testkit::kperf]
    fn user_1_page(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::User(1), 1);
    }

    #[testkit::kperf]
    fn user_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::User(1), 16);
    }

    #[testkit::kperf]
    fn user_256_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_allocate_free(perf, FrameOwner::User(1), 256);
    }

    #[testkit::kperf]
    fn contiguous_kernel_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_contiguous(perf, FrameOwner::Kernel, 16);
    }

    #[testkit::kperf]
    fn contiguous_kernel_256_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_contiguous(perf, FrameOwner::Kernel, 256);
    }

    #[testkit::kperf]
    fn contiguous_user_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_contiguous(perf, FrameOwner::User(1), 16);
    }

    #[testkit::kperf]
    fn contiguous_user_256_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_contiguous(perf, FrameOwner::User(1), 256);
    }

    #[testkit::kperf]
    fn fragmented_kernel_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_fragmented(perf, FrameOwner::Kernel);
    }

    #[testkit::kperf]
    fn fragmented_user_16_pages(perf: &mut Perf) {
        initialize();
        cases::frame_allocator::perf_fragmented(perf, FrameOwner::User(1));
    }
}

mod frame {
    use super::*;

    #[testkit::kperf]
    fn reference(perf: &mut Perf) {
        initialize();
        cases::frame::perf_reference(perf);
    }

    #[testkit::kperf]
    fn write_access(perf: &mut Perf) {
        initialize();
        cases::frame::perf_access(perf, FrameAccess::Write);
    }

    #[testkit::kperf]
    fn execute_access(perf: &mut Perf) {
        initialize();
        cases::frame::perf_access(perf, FrameAccess::Execute);
    }
}

mod heap_allocator {
    use super::*;

    #[testkit::kperf]
    fn cached_64_bytes(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_cached(perf, 64);
    }

    #[testkit::kperf]
    fn cached_2048_bytes(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_cached(perf, 2048);
    }

    #[testkit::kperf]
    fn grow_reclaim_128_objects(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_grow_reclaim(perf);
    }

    #[testkit::kperf]
    fn large_4_kib(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_large(perf, 4096);
    }

    #[testkit::kperf]
    fn large_64_kib(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_large(perf, 65536);
    }

    #[testkit::kperf]
    fn mixed_48_objects(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_mixed(perf);
    }

    #[testkit::kperf]
    fn box_64_bytes(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_box::<64>(perf);
    }

    #[testkit::kperf]
    fn box_2048_bytes(perf: &mut Perf) {
        initialize();
        cases::heap_allocator::perf_box::<2048>(perf);
    }
}

mod address_space {
    use super::*;

    #[testkit::kperf]
    fn create_drop(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_create_drop(perf);
    }

    #[testkit::kperf]
    fn map_unmap_1_page(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_map_unmap(perf, 1);
    }

    #[testkit::kperf]
    fn map_unmap_64_pages(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_map_unmap(perf, 64);
    }

    #[testkit::kperf]
    fn protect_rw_ro_rw(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_protect(perf, PagePermissions::ReadOnly);
    }

    #[testkit::kperf]
    fn protect_rw_rx_rw(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_protect(perf, PagePermissions::ReadExecute);
    }

    #[testkit::kperf]
    fn query_4_hits_16_entries(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_query(perf, 16, true);
    }

    #[testkit::kperf]
    fn query_4_holes_16_entries(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_query(perf, 16, false);
    }

    #[testkit::kperf]
    fn query_4_hits_256_entries(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_query(perf, 256, true);
    }

    #[testkit::kperf]
    fn query_4_holes_256_entries(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_query(perf, 256, false);
    }

    #[testkit::kperf]
    fn reserve_release_4_pages_16_ranges(perf: &mut Perf) {
        initialize();
        cases::address_space::perf_reserve_release(perf);
    }
}

mod stack {
    use super::*;

    #[testkit::kperf]
    fn create_drop_4_pages(perf: &mut Perf) {
        initialize();
        cases::stack::perf_create_drop(perf);
    }
}
