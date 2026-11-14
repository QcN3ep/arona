# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

from pathlib import Path
import subprocess
import re
import shutil

SLICE = 16 * 1024 * 1024


def create_volume(directory):
    directory.mkdir(parents=True, exist_ok=True)
    tree = directory / "root"
    data = tree / "data"
    data.mkdir(parents=True, exist_ok=True)
    (data / "message").write_bytes(bytes(index % 251 for index in range(12000)))
    (data / "sparse").write_bytes(bytes(16384))
    (data / "tree").write_bytes(b"")
    with open(bytes(data) + b"/\xff", "wb") as output:
        output.write(b"raw")
    volume = directory / "volume.ext4"
    with volume.open("wb") as output:
        output.truncate(SLICE)
    subprocess.run(["mke2fs", "-q", "-F", "-t", "ext4", "-b", "4096", "-I", "256",
                    "-O", "none,has_journal,extent,filetype,sparse_super,large_file", "-E", "lazy_itable_init=0",
                    "-d", str(tree), str(volume)], check=True)
    for index in range(0, 12, 2):
        subprocess.run(["debugfs", "-w", "-R", f"fallocate /data/tree {index} {index}", str(volume)], check=True, capture_output=True)
    subprocess.run(["debugfs", "-w", "-R", "set_inode_field /data/tree size 49152", str(volume)], check=True, capture_output=True)
    journal_blocks = subprocess.run(["debugfs", "-R", "blocks <8>", str(volume)], check=True, capture_output=True, text=True)
    first_journal = int(journal_blocks.stdout.split()[0])
    with volume.open("r+b") as output:
        output.seek(first_journal * 4096 + 24)
        output.write((0xfffffffe).to_bytes(4, "big"))
    return volume


def create_image(directory):
    good = create_volume(directory).read_bytes()
    variants = []
    for offset, value in ((1024 + 58, b"\x02\x00"), (1024 + 96, (0x8042).to_bytes(4, "little")),
                          (4096 + 8, (0xffffffff).to_bytes(4, "little")),
                          (1024 + 4, (0xffffffff).to_bytes(4, "little"))):
        bad = bytearray(good)
        bad[offset:offset + len(value)] = value
        variants.append(bad)
    inode_table = int.from_bytes(good[4104:4108], "little") * 4096
    root_inode = inode_table + 256
    bad_extent = bytearray(good)
    bad_extent[root_inode + 60:root_inode + 64] = (0xffffffff).to_bytes(4, "little")
    variants.append(bad_extent)
    root_block = int.from_bytes(good[root_inode + 60:root_inode + 64], "little") * 4096
    bad_directory = bytearray(good)
    bad_directory[root_block + 4:root_block + 6] = bytes(2)
    variants.append(bad_directory)
    image = directory / "disk.raw"
    with image.open("wb") as output:
        output.write(good)
        for bad in variants:
            output.write(bad)
    return image


def create_persistence_images(directory):
    disk = create_image(directory)
    with disk.open("rb") as source:
        good = source.read(SLICE)
    with disk.open("ab") as output:
        output.write(good)
    orphan = directory / "orphan.ext4"
    orphan.write_bytes(good)
    def debug(command):
        return subprocess.run(["debugfs", "-w", "-R", command, str(orphan)], check=True, capture_output=True, text=True).stdout
    debug(f"write {directory / 'root/data/message'} /orphan")
    number = int(re.search(r"Inode: (\d+)", debug("stat /orphan")).group(1))
    debug("unlink /orphan")
    debug(f"set_inode_field <{number}> links_count 0")
    debug(f"set_super_value last_orphan {number}")
    orphan_bytes = bytearray(orphan.read_bytes())
    orphan_bytes[1082:1084] = bytes(2)
    orphan_bytes[1120:1124] = (int.from_bytes(orphan_bytes[1120:1124], "little") | 4).to_bytes(4, "little")
    plain = directory / "plain.ext4"
    plain.write_bytes(good)
    subprocess.run(["tune2fs", "-O", "^has_journal", str(plain)], check=True, capture_output=True)
    with disk.open("ab") as output:
        output.write(orphan_bytes)
        output.write(plain.read_bytes())
    for name, slices in (("ro", 1), ("write", 2), ("flush", 3), ("prepare", 5)):
        with (directory / f"{name}.raw").open("wb") as output:
            output.write(good)
            output.truncate(slices * SLICE)
    blocks = subprocess.run(["debugfs", "-R", "blocks /data/message", str(directory / "volume.ext4")], check=True, capture_output=True, text=True)
    sector = int(blocks.stdout.split()[0]) * 8
    (directory / "write.conf").write_text(
        f'[inject-error]\nevent = "write_aio"\nsector = "{sector}"\nerrno = "5"\nonce = "on"\n'
    )
    (directory / "flush.conf").write_text(
        '[set-state]\nevent = "flush_to_disk"\nstate = "1"\nnew_state = "2"\n'
        '[set-state]\nevent = "flush_to_disk"\nstate = "2"\nnew_state = "3"\n'
        '[inject-error]\nevent = "flush_to_disk"\nstate = "3"\nerrno = "5"\nonce = "on"\n'
    )
    (directory / "prepare.conf").write_text(
        '[set-state]\nevent = "flush_to_disk"\nstate = "1"\nnew_state = "2"\n'
        '[inject-error]\nevent = "flush_to_disk"\nstate = "2"\nerrno = "5"\nonce = "on"\n'
    )
    return disk


def check_pending_journal(directory):
    pending = directory / "write.raw"
    blocks = subprocess.run(["debugfs", "-R", "blocks <8>", str(pending)], check=True, capture_output=True, text=True)
    journal = [int(value) for value in blocks.stdout.split()]
    image = bytearray(pending.read_bytes())
    original = bytes(image)
    offset = journal[0] * 4096
    first = int.from_bytes(image[offset + 20:offset + 24], "big")
    length = int.from_bytes(image[offset + 16:offset + 20], "big")
    new_start = length - 2
    for index in range(first, length):
        destination = first + (new_start - first + index - first) % (length - first)
        image[journal[destination] * 4096:(journal[destination] + 1) * 4096] = original[journal[index] * 4096:(journal[index] + 1) * 4096]
    image[offset + 28:offset + 32] = new_start.to_bytes(4, "big")
    image[offset + 252:offset + 256] = bytes(4)
    crc = 0xffffffff
    for byte in image[offset:offset + 1024]:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82f63b78 if crc & 1 else 0)
    image[offset + 252:offset + 256] = crc.to_bytes(4, "big")
    pending.write_bytes(image)
    replay = directory / "replay.raw"
    shutil.copyfile(pending, replay)
    with replay.open("r+b") as output:
        output.truncate(6 * SLICE)
    linux = directory / "linux-recovered.ext4"
    with pending.open("rb") as source:
        linux.write_bytes(source.read(SLICE))
    result = subprocess.run(["e2fsck", "-fy", str(linux)])
    assert result.returncode in (0, 1), "e2fsck could not replay the JBD2 transaction"
    subprocess.run(["e2fsck", "-fn", str(linux)], check=True)
    blocks = subprocess.run(["debugfs", "-R", "blocks <8>", str(pending)], check=True, capture_output=True, text=True)
    journal = [int(value) for value in blocks.stdout.split()]
    damaged = directory / "damaged.raw"
    shutil.copyfile(pending, damaged)
    with damaged.open("r+b") as output:
        output.seek(journal[length - 1] * 4096)
        byte = output.read(1)
        output.seek(-1, 1)
        output.write(bytes([byte[0] ^ 1]))
        output.truncate(4 * SLICE)


def check_persistence_images(directory, disk):
    with disk.open("rb") as source:
        volume = source.read(SLICE)
        source.seek(7 * SLICE + 1082)
        assert source.read(2) == b"\x01\x00", "abandoned volume was not recovered"
    assert volume[1082:1084] == b"\x01\x00", "unmounted volume remains dirty"
    checked = directory / "checked.ext4"
    checked.write_bytes(volume)
    subprocess.run(["e2fsck", "-fn", str(checked)], check=True)
    metadata = subprocess.run(
        ["debugfs", "-R", "stat /links/alias", str(checked)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    assert "Links: 1" in metadata and "User: 305419896" in metadata
    assert "Group: -1412628189" in metadata and "Mode:  06751" in metadata
    short_link = subprocess.run(
        ["debugfs", "-R", "stat /links/short", str(checked)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    assert 'Fast link dest: "../links/alias"' in short_link
    long_link = subprocess.run(
        ["debugfs", "-R", "cat /links/long", str(checked)],
        check=True,
        capture_output=True,
    ).stdout
    assert long_link == b"x" * 200
    for name in ("write", "flush", "prepare", "replay"):
        with (directory / f"{name}.raw").open("rb") as source:
            source.seek(1082)
            assert source.read(2) == b"\x01\x00", f"{name} recovery remains dirty"
            source.seek(0)
            checked.write_bytes(source.read(SLICE))
        subprocess.run(["e2fsck", "-fn", str(checked)], check=True)
    with disk.open("rb") as source:
        source.seek(8 * SLICE)
        checked.write_bytes(source.read(SLICE))
    subprocess.run(["e2fsck", "-fn", str(checked)], check=True)


def main():
    import argparse

    parser = argparse.ArgumentParser(description="Prepare and inspect ext4 test volumes")
    parser.add_argument("action", choices=["volume", "prepare", "recover", "check"])
    parser.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    directory = arguments.directory
    if arguments.action == "volume":
        create_volume(directory)
    elif arguments.action == "prepare":
        create_persistence_images(directory)
    elif arguments.action == "recover":
        check_pending_journal(directory)
    else:
        check_persistence_images(directory, directory / "disk.raw")


if __name__ == "__main__":
    main()
