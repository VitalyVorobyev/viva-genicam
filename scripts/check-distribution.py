#!/usr/bin/env python3
"""Inspect prepared zip/wheel/tar archives without extracting them."""
import argparse
from pathlib import Path, PurePosixPath
import stat
import sys
import tarfile
import zipfile


def is_notice(name):
    name = PurePosixPath(name.replace("\\", "/")).name.lower()
    return "third_party_notices" in name or "third-party-notices" in name


def check_names(names):
    notices = False
    for raw in names:
        path = PurePosixPath(raw.replace("\\", "/"))
        parts = {part.lower() for part in path.parts}
        name = path.name.lower()
        if path.is_absolute() or ".." in parts:
            raise ValueError(f"Unsafe archive path: {raw}")
        normalized = "/".join(part.lower() for part in path.parts)
        if (parts & {".local-evidence", "vendor-xml", ".claude", ".codegraph"}
                or "target/hardware/" in normalized or "target/legal/" in normalized
                or name.endswith((".pcap", ".pcapng"))
                or name in {"gentl.h", "genapischema_version_1_1.xsd", "viva-report.txt", "report-before.txt"}):
            raise ValueError(f"Private or fetched development material in archive: {raw}")
        notices |= is_notice(name)
    if not notices:
        raise ValueError("Archive has no THIRD_PARTY_NOTICES file")


def check_notice(contents):
    if b"Generated from locked, target-filtered dependency metadata" not in contents:
        raise ValueError("Dependency notices are a development placeholder or were not generated")


def check_archive(path):
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            check_names(archive.namelist())
            notices = [member for member in archive.infolist()
                       if not member.is_dir() and is_notice(member.filename)
                       and stat.S_IFMT(member.external_attr >> 16) in (0, stat.S_IFREG)]
            if not notices:
                raise ValueError("Archive has no regular dependency notice file")
            for member in notices:
                check_notice(archive.read(member))
    else:
        with tarfile.open(path) as archive:
            check_names(archive.getnames())
            notices = [member for member in archive.getmembers() if member.isfile() and is_notice(member.name)]
            if not notices:
                raise ValueError("Archive has no regular dependency notice file")
            for member in notices:
                check_notice(archive.extractfile(member).read())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", nargs="+", type=Path)
    args = parser.parse_args()
    for path in args.archives:
        check_archive(path)
        print(f"{path}: notices present, no private/fetched development material")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as error:
        sys.exit(str(error))
