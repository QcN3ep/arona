# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

"""Create standard FAT32 volumes with mtools."""

from pathlib import Path
import os
import shutil
import subprocess
import sys

os.environ["LC_ALL"] = "C.UTF-8"

SIZE = 64 * 1024 * 1024


def prepare(directory):
    directory.mkdir(parents=True, exist_ok=True)
    image = directory / "fat32.raw"
    with image.open("wb") as output:
        output.truncate(SIZE)
    subprocess.run(["mformat", "-i", str(image), "-F", "-c", "1", "::"], check=True)
    message = directory / "message"
    message.write_bytes(bytes(index % 251 for index in range(12000)))
    subprocess.run(["mmd", "-i", str(image), "::/data"], check=True)
    for name in ["Short.bin", "Long filename café.txt"]:
        subprocess.run(["mcopy", "-i", str(image), str(message), "::/data/" + name], check=True)
    shutil.copyfile(image, directory / "readonly.raw")
    good = image.read_bytes()
    invalid = bytearray(good)
    invalid[13] = 0
    cycle = bytearray(good)
    fat = int.from_bytes(good[14:16], "little") * 512
    root = int.from_bytes(good[44:48], "little")
    cycle[fat + root * 4:fat + root * 4 + 4] = root.to_bytes(4, "little")
    full = directory / "full.raw"
    full.write_bytes(good)
    fat_size = int.from_bytes(good[36:40], "little") * 512
    fat_bytes = good[fat:fat + fat_size]
    total = int.from_bytes(good[32:36], "little")
    clusters = total - int.from_bytes(good[14:16], "little") - good[16] * (fat_size // 512)
    free = sum(int.from_bytes(fat_bytes[index * 4:index * 4 + 4], "little") == 0 for index in range(2, clusters + 2))
    filler = directory / "filler"
    with filler.open("wb") as output:
        output.truncate((free - 4) * 512)
    subprocess.run(["mcopy", "-i", str(full), str(filler), "::/filler"], check=True)
    (directory / "faterror.raw").write_bytes(good)
    (directory / "faterror.conf").write_text('[inject-error]\nevent = "write_aio"\nerrno = "5"\nimmediately = "on"\nonce = "on"\n')
    with (directory / "fatflush.raw").open("wb") as output:
        output.write(good)
        output.truncate(2 * SIZE)
    (directory / "fatflush.conf").write_text('[inject-error]\nevent = "flush_to_disk"\nerrno = "5"\nimmediately = "on"\nonce = "on"\n')
    wide = directory / "wide.raw"
    with wide.open("wb") as output:
        output.truncate(8 * SIZE)
    subprocess.run(["mformat", "-i", str(wide), "-F", "-c", "1", "-S", "5", "-T", str(8 * SIZE // 4096), "::"], check=True)
    subprocess.run(["mcopy", "-i", str(wide), str(message), "::/wide.bin"], check=True)
    active = bytearray(good)
    active[40:42] = (0x81).to_bytes(2, "little")
    active[fat + root * 4:fat + root * 4 + 4] = root.to_bytes(4, "little")
    with image.open("ab") as output:
        output.write(invalid)
        output.write(cycle)
        output.write(full.read_bytes())
        output.write(active)
        with wide.open("rb") as source:
            shutil.copyfileobj(source, output)


def check(directory):
    image = directory / "fat32.raw"
    result = directory / "persisted"
    result.unlink(missing_ok=True)
    subprocess.run(["mcopy", "-i", str(image), "::/persist.bin", str(result)], check=True)
    assert result.read_bytes() == bytes(index % 251 for index in range(12000))
    with image.open("rb") as source:
        boot = source.read(512)
        first = int.from_bytes(boot[14:16], "little") * 512
        size = int.from_bytes(boot[36:40], "little") * 512
        source.seek(first)
        assert source.read(size) == source.read(size), "mirrored FATs differ"
    subprocess.run(["mdir", "-i", str(image), "::/"], check=True)
    result.unlink()
    subprocess.run(["mcopy", "-i", f"{image}@@{5 * SIZE}", "::/wide-write.bin", str(result)], check=True)
    assert result.read_bytes() == b"wide sectors"
    result.unlink()
    subprocess.run(["mcopy", "-i", str(image), "::/SERVIC~1.TXT", str(result)], check=True)
    assert result.read_bytes() == b"unicode"
    # mtools substitutes surrogate pairs. Check the disk name with Python's UTF-16 decoder.
    with image.open("rb") as source:
        sector = int.from_bytes(boot[11:13], "little")
        cluster_size = boot[13] * sector
        root = int.from_bytes(boot[44:48], "little")
        data = first + boot[16] * size + (root - 2) * cluster_size
        source.seek(data)
        names = {}
        found = False
        for _ in range(cluster_size // 32):
            raw = source.read(32)
            if raw[0] == 0:
                raise AssertionError("Unicode directory entry not found")
            if raw[11] == 15:
                names[raw[0] & 31] = b"".join(raw[at:at + 2] for at in [1,3,5,7,9,14,16,18,20,22,24,28,30])
            elif raw[:11] == b"SERVIC~1TXT":
                text = b"".join(names[index] for index in sorted(names)).decode("utf-16-le").split("\0")[0]
                assert text == "service λ𐐀.txt"
                found = True
                break
            else:
                names.clear()
        assert found, "Unicode directory entry not found"


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(Path(sys.argv[2]))
    elif sys.argv[1] == "check":
        check(Path(sys.argv[2]))
    else:
        raise SystemExit("unknown FAT32 fixture action")
