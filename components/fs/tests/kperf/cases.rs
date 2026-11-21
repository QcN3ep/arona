// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Apply the same observable operations to each filesystem backend.

macro_rules! cases {
    ($filesystem:ty) => {
        use fs::ReadAt;
        use testkit::kperf;

        fn finish(filesystem: $filesystem, driver: Option<disk::Driver>) {
            filesystem.unmount().unwrap();
            drop(filesystem);
            drop(driver);
        }

        fn read(perf: &mut Perf, size: usize) {
            let (filesystem, driver) = setup();
            let file = filesystem.create(b"/sample").unwrap();
            let bytes = alloc::vec![0x5a; size];
            assert_eq!(file.write_at(0, &bytes).unwrap(), size);
            file.fsync().unwrap();
            let mut output = alloc::vec![0; size];
            file.read_exact_at(0, &mut output).unwrap();
            perf.iter(|| file.read_exact_at(0, core::hint::black_box(&mut output)).unwrap());
            assert_eq!(output, bytes);
            drop(file);
            finish(filesystem, driver);
        }

        /// Read through an already open file after warming its data and metadata.
        #[kperf]
        fn read_warm_4kib(perf: &mut Perf) { read(perf, 4096); }

        #[kperf]
        fn read_warm_64kib(perf: &mut Perf) { read(perf, 65536); }

        /// Overwrite the same allocated range and include durability in each iteration.
        #[kperf]
        fn overwrite_sync_4kib(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            let file = filesystem.create(b"/sample").unwrap();
            let bytes = alloc::vec![0x5a; 4096];
            assert_eq!(file.write_at(0, &bytes).unwrap(), bytes.len());
            file.fsync().unwrap();
            let mut count = 0;
            perf.iter(|| {
                count = file.write_at(0, core::hint::black_box(&bytes)).unwrap();
                file.fsync().unwrap();
            });
            assert_eq!(count, bytes.len());
            let mut output = alloc::vec![0; bytes.len()];
            file.read_exact_at(0, &mut output).unwrap();
            assert_eq!(output, bytes);
            drop(file);
            finish(filesystem, driver);
        }

        /// Grow one file and include allocation, metadata, and durability in each iteration.
        #[kperf]
        fn append_sync_4kib(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            let file = filesystem.create(b"/sample").unwrap();
            let bytes = alloc::vec![0x5a; 4096];
            let mut length = 0;
            perf.iter(|| {
                let (offset, count) = file.append(core::hint::black_box(&bytes)).unwrap();
                length = offset + count;
                file.fsync().unwrap();
            });
            assert_eq!(file.len(), length);
            let mut output = alloc::vec![0; bytes.len()];
            file.read_exact_at(length - bytes.len(), &mut output).unwrap();
            assert_eq!(output, bytes);
            drop(file);
            finish(filesystem, driver);
        }

        #[kperf]
        fn metadata_open(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            let file = filesystem.create(b"/sample").unwrap();
            let expected = file.metadata().unwrap();
            let mut output = expected;
            perf.iter(|| output = file.metadata().unwrap());
            assert_eq!(output, expected);
            drop(file);
            finish(filesystem, driver);
        }

        /// Resolve a nested path, open the node, and release the open handle.
        #[kperf]
        fn open_close(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            filesystem.mkdir(b"/work").unwrap();
            let expected = filesystem.create(b"/work/sample").unwrap().metadata().unwrap();
            let mut output = expected;
            perf.iter(|| output = filesystem.open_bytes(core::hint::black_box(b"/work/sample")).unwrap().metadata().unwrap());
            assert_eq!(output, expected);
            finish(filesystem, driver);
        }

        #[kperf]
        fn directory_32(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            filesystem.mkdir(b"/work").unwrap();
            for index in 0..32 {
                drop(filesystem.create(alloc::format!("/work/file-{index:02}").as_bytes()).unwrap());
            }
            let mut entries = alloc::vec::Vec::new();
            perf.iter(|| entries = filesystem.read_dir(core::hint::black_box(b"/work")).unwrap());
            assert_eq!(entries.len(), 32);
            for index in 0..32 {
                assert!(entries.iter().any(|entry| entry.name == alloc::format!("file-{index:02}").as_bytes()));
            }
            finish(filesystem, driver);
        }

        /// Create and remove one empty file, then synchronize its parent directory.
        #[kperf]
        fn create_remove_sync(perf: &mut Perf) {
            let (filesystem, driver) = setup();
            filesystem.mkdir(b"/work").unwrap();
            let directory = filesystem.open_directory(b"/work").unwrap();
            perf.iter(|| {
                let file = filesystem.create(b"/work/sample").unwrap();
                file.fsync().unwrap();
                drop(file);
                filesystem.unlink(b"/work/sample").unwrap();
                directory.fsync().unwrap();
            });
            assert!(directory.read_dir().unwrap().is_empty());
            drop(directory);
            finish(filesystem, driver);
        }
    };
}
