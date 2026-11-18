# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

"""Create standard FAT32 volumes with mtools."""

from pathlib import Path
import shutil
import subprocess
import sys

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
    with image.open("ab") as output:
        output.write(invalid)
        output.write(cycle)


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(Path(sys.argv[2]))
    else:
        raise SystemExit("unknown FAT32 fixture action")
