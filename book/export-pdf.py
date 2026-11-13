#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

"""Add PDF bookmarks and export the finished handbook."""

import json
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    raw = sys.stdin.buffer.read()
    context = json.loads(raw)
    subprocess.run(["mdbook-pdf-outline"], input=raw, check=True)

    destination = Path(context["destination"])
    exports = (
        Path(context["root"]).parent / "docs" / "arona-book.pdf",
        destination.parent / "html" / "docs" / "arona-book.pdf",
    )
    for path in exports:
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(destination / "output.pdf", path)


if __name__ == "__main__":
    main()
