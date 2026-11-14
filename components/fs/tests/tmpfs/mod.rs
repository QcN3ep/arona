// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Host tests for the in-memory filesystem.

use fs::{
    Attributes, Error, FileType, ReadAt, Timestamp,
    tmpfs::{File, Tmpfs},
};

fn read(file: &File) -> Vec<u8> {
    let mut bytes = vec![0; file.len()];
    file.read_at(0, &mut bytes).unwrap();
    bytes
}

fn names(entries: &[fs::DirEntry]) -> Vec<&[u8]> {
    entries.iter().map(|entry| entry.name.as_slice()).collect()
}

#[test]
fn create_write_read_round_trip() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    assert_eq!(file.write_at(0, b"hello").unwrap(), 5);
    assert_eq!(file.len(), 5);
    assert_eq!(read(&file), b"hello".to_vec());

    assert_eq!(file.append(b" world").unwrap(), (5, 6));
    assert_eq!(file.len(), 11);
    assert_eq!(read(&file), b"hello world".to_vec());
    assert_eq!(tmpfs.metadata(b"/a").unwrap().size, 11);

    file.truncate(2).unwrap();
    assert_eq!(file.len(), 2);
    assert_eq!(read(&file), b"he".to_vec());

    file.truncate(4).unwrap();
    assert_eq!(read(&file), b"he\0\0".to_vec());
}

#[test]
fn writes_beyond_the_file_are_zero_filled() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    file.write_at(4, b"xy").unwrap();
    assert_eq!(file.len(), 6);
    assert_eq!(read(&file), b"\0\0\0\0xy".to_vec());
}

#[test]
fn directories_list_in_name_order() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/d").unwrap();
    tmpfs.mkdir(b"/d/e").unwrap();
    tmpfs.create(b"/d/f").unwrap();
    let entries = tmpfs.read_dir(b"/d").unwrap();
    assert_eq!(names(&entries), [b"e".as_slice(), b"f".as_slice()]);
    assert_eq!(entries[0].kind, FileType::Directory);
    assert_eq!(entries[1].kind, FileType::Regular);
    assert!(entries.iter().all(|entry| entry.inode != 0));
    assert!(tmpfs.read_dir(b"/d/e").unwrap().is_empty());
    assert_eq!(
        tmpfs.metadata(b"/d").unwrap().links,
        3,
        "a directory counts its parent and its subdirectory"
    );
}

#[test]
fn directory_operations_report_their_errors() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/d").unwrap();
    assert_eq!(tmpfs.mkdir(b"/d").unwrap_err(), Error::AlreadyExists);
    assert_eq!(tmpfs.create(b"/d").err(), Some(Error::AlreadyExists));
    assert_eq!(tmpfs.mkdir(b"/missing/e").unwrap_err(), Error::NotFound);
    assert_eq!(tmpfs.create(b"/missing/file").err(), Some(Error::NotFound));
    assert_eq!(tmpfs.read_dir(b"/nowhere").unwrap_err(), Error::NotFound);
    assert_eq!(tmpfs.open_bytes(b"/d").err(), Some(Error::IsDirectory));
    assert_eq!(
        tmpfs.open_directory(b"/nowhere").err(),
        Some(Error::NotFound)
    );
    tmpfs.create(b"/f").unwrap();
    assert_eq!(tmpfs.open_directory(b"/f").err(), Some(Error::NotDirectory));
    assert_eq!(tmpfs.read_dir(b"/f").unwrap_err(), Error::NotDirectory);
}

#[test]
fn rename_moves_and_replaces_entries() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    file.write_at(0, b"data").unwrap();
    tmpfs.rename(b"/a", b"/b").unwrap();
    assert_eq!(tmpfs.metadata(b"/a").unwrap_err(), Error::NotFound);
    assert_eq!(tmpfs.metadata(b"/b").unwrap().size, 4);

    tmpfs.create(b"/c").unwrap();
    tmpfs.rename(b"/b", b"/c").unwrap();
    assert_eq!(read(&tmpfs.open_bytes(b"/c").unwrap()), b"data".to_vec());
    assert_eq!(
        tmpfs.rename(b"/c", b"/missing/d").unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        tmpfs.rename(b"/missing", b"/d").unwrap_err(),
        Error::NotFound
    );
}

#[test]
fn rename_rejects_directories_beneath_themselves() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/a").unwrap();
    tmpfs.mkdir(b"/a/b").unwrap();
    assert_eq!(
        tmpfs.rename(b"/a", b"/a/b/c").unwrap_err(),
        Error::InvalidRequest
    );
}

#[test]
fn rename_rejects_replacing_a_non_empty_directory() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/a").unwrap();
    tmpfs.mkdir(b"/b").unwrap();
    tmpfs.create(b"/b/f").unwrap();
    assert_eq!(tmpfs.rename(b"/a", b"/b").unwrap_err(), Error::NotEmpty);
}

#[test]
fn unlink_removes_entries_and_rejects_inhabited_directories() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/d").unwrap();
    tmpfs.create(b"/d/f").unwrap();
    assert_eq!(tmpfs.unlink(b"/d").unwrap_err(), Error::NotEmpty);
    tmpfs.unlink(b"/d/f").unwrap();
    assert_eq!(tmpfs.metadata(b"/d/f").unwrap_err(), Error::NotFound);
    tmpfs.unlink(b"/d").unwrap();
    assert_eq!(tmpfs.metadata(b"/d").unwrap_err(), Error::NotFound);
    assert_eq!(tmpfs.unlink(b"/d").unwrap_err(), Error::NotFound);
}

#[test]
fn open_files_survive_unlink_until_they_close() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    file.write_at(0, b"kept").unwrap();
    tmpfs.unlink(b"/a").unwrap();
    assert_eq!(tmpfs.metadata(b"/a").unwrap_err(), Error::NotFound);
    assert_eq!(read(&file), b"kept".to_vec());
    drop(file);
}

#[test]
fn hard_links_share_content_and_count_links() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    tmpfs.hard_link(b"/a", b"/b").unwrap();
    assert_eq!(tmpfs.metadata(b"/b").unwrap().links, 2);
    file.write_at(0, b"shared").unwrap();
    assert_eq!(read(&tmpfs.open_bytes(b"/b").unwrap()), b"shared".to_vec());

    tmpfs.mkdir(b"/d").unwrap();
    assert_eq!(
        tmpfs.hard_link(b"/d", b"/e").unwrap_err(),
        Error::IsDirectory
    );
    assert_eq!(
        tmpfs.hard_link(b"/a", b"/b").unwrap_err(),
        Error::AlreadyExists
    );
    assert_eq!(
        tmpfs.hard_link(b"/missing", b"/c").unwrap_err(),
        Error::NotFound
    );
    tmpfs.unlink(b"/a").unwrap();
    assert_eq!(tmpfs.metadata(b"/b").unwrap().links, 1);
}

#[test]
fn symbolic_links_resolve_and_report_their_target() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.create(b"/a").unwrap();
    tmpfs.symlink(b"a", b"/s").unwrap();
    let metadata = tmpfs.metadata(b"/s").unwrap();
    assert_eq!(metadata.mode & 0o170000, 0o120000);
    assert_eq!(metadata.size, 1);
    assert_eq!(tmpfs.read_link(b"/s").unwrap(), b"a".to_vec());
    assert_eq!(tmpfs.read_link(b"/a").unwrap_err(), Error::InvalidRequest);
    assert_eq!(tmpfs.open_bytes(b"/s").err(), Some(Error::InvalidRequest));
    assert_eq!(
        tmpfs.symlink(b"", b"/t").unwrap_err(),
        Error::InvalidRequest
    );
    assert_eq!(
        tmpfs.symlink(b"a", b"/a").unwrap_err(),
        Error::AlreadyExists
    );
}

#[test]
fn capacity_limits_writes_and_reports_free_space() {
    let tmpfs = Tmpfs::with_capacity(8192).unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    let statistics = tmpfs.statistics().unwrap();
    assert_eq!(statistics.block_size, 4096);
    assert_eq!(statistics.blocks, 2);

    file.write_at(0, &vec![7; 8192]).unwrap();
    assert_eq!(tmpfs.statistics().unwrap().free_blocks, 0);
    assert_eq!(file.write_at(8192, b"x").unwrap_err(), Error::NoSpace);
    assert_eq!(
        tmpfs.create(b"/b").unwrap().truncate(4096).unwrap_err(),
        Error::NoSpace
    );
    assert!(matches!(
        Tmpfs::with_capacity(100),
        Err(Error::InvalidRequest)
    ));
}

#[test]
fn statistics_describe_the_mount() {
    let tmpfs = Tmpfs::new().unwrap();
    let statistics = tmpfs.statistics().unwrap();
    assert_eq!(statistics.block_size, 4096);
    assert_eq!(statistics.blocks, 16384);
    assert_eq!(statistics.free_blocks, statistics.blocks);
    assert!(statistics.files >= 1);
    assert_eq!(statistics.name_length, 255);

    let metadata = tmpfs.metadata(b"/").unwrap();
    assert_ne!(metadata.device, 0);
    assert_ne!(metadata.mount, 0);
    assert_eq!(metadata.mode, 0o040755);
    assert_eq!(metadata.links, 2);

    let other = Tmpfs::new().unwrap();
    assert_ne!(other.metadata(b"/").unwrap().mount, metadata.mount);
}

#[test]
fn attributes_are_stored_and_validated() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    let time = Timestamp {
        seconds: 42,
        nanoseconds: 7,
    };
    file.set_attributes(Attributes {
        permissions: 0o600,
        uid: 1000,
        gid: 100,
        accessed: time,
        modified: time,
        changed: time,
    })
    .unwrap();
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.mode, 0o100600);
    assert_eq!(metadata.uid, 1000);
    assert_eq!(metadata.gid, 100);
    assert_eq!(metadata.modified, time);

    assert_eq!(
        file.set_attributes(Attributes {
            permissions: 0o10000,
            ..Attributes::default()
        })
        .unwrap_err(),
        Error::InvalidRequest
    );
}

#[test]
fn directories_report_their_relative_path() {
    let tmpfs = Tmpfs::new().unwrap();
    tmpfs.mkdir(b"/d").unwrap();
    tmpfs.mkdir(b"/d/e").unwrap();
    assert!(
        tmpfs
            .open_directory(b"/")
            .unwrap()
            .path()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        tmpfs.open_directory(b"/d/e").unwrap().path().unwrap(),
        b"d/e".to_vec()
    );
    assert_eq!(
        tmpfs
            .open_directory(b"/d/e")
            .unwrap()
            .parent_metadata()
            .unwrap()
            .unwrap()
            .mode,
        0o040755
    );
}

#[test]
fn unmount_waits_for_open_files() {
    let tmpfs = Tmpfs::new().unwrap();
    let file = tmpfs.create(b"/a").unwrap();
    assert_eq!(tmpfs.unmount().unwrap_err(), Error::Busy);
    drop(file);
    tmpfs.unmount().unwrap();
}
