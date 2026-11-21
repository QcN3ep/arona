# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

"""Create reproducible empty filesystems for performance cases."""

import os
from pathlib import Path
import subprocess
import sys

kind, destination = sys.argv[1:]
image = Path(destination)
with image.open("wb") as output:
    output.truncate(64 * 1024 * 1024)
if kind == "ext4":
    environment = dict(os.environ, E2FSPROGS_FAKE_TIME="946684800")
    subprocess.run([
        "mke2fs", "-q", "-F", "-t", "ext4", "-b", "4096", "-I", "256",
        "-O", "none,has_journal,extent,filetype,sparse_super,large_file,64bit,metadata_csum,flex_bg,huge_file,dir_nlink,extra_isize",
        "-U", "704a1fe8-70a7-41ad-84bc-6f564501f987",
        "-E", "lazy_itable_init=0,hash_seed=704a1fe8-70a7-41ad-84bc-6f564501f987",
        str(image),
    ], env=environment, check=True)
elif kind == "fat32":
    subprocess.run(["mformat", "-i", str(image), "-F", "-c", "1", "-N", "704a1fe8", "::"], check=True)
else:
    raise ValueError(kind)
