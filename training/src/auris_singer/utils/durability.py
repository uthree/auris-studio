"""Durability primitives for atomically published training artifacts."""

from __future__ import annotations

import errno
import os
from pathlib import Path

__all__ = ["fsync_directory", "fsync_file", "fsync_tree"]


def fsync_file(path: str | Path) -> None:
    """Flush one completed regular file to stable storage."""
    # Windows' C runtime rejects ``fsync`` on a read-only descriptor even
    # though no write is performed here. Published candidates are private and
    # writable, so open them read/write on every platform for one code path.
    with Path(path).open("r+b") as stream:
        os.fsync(stream.fileno())


def _directory_sync_is_unsupported(error: OSError) -> bool:
    """Whether this platform cannot open or flush directory handles."""
    unsupported = {errno.EINVAL, errno.ENOTSUP, errno.EISDIR}
    if error.errno in unsupported:
        return True
    # Python cannot portably request ``FILE_FLAG_BACKUP_SEMANTICS`` when it
    # opens a directory on Windows. NTFS still receives durable file flushes;
    # directory flushing is therefore best effort only on that platform.
    return os.name == "nt" and (
        error.errno in {errno.EACCES, errno.EPERM} or getattr(error, "winerror", None) in {5, 87}
    )


def fsync_directory(path: str | Path) -> None:
    """Flush directory entries, failing unless the operation is unsupported."""
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    try:
        descriptor = os.open(Path(path), flags)
    except OSError as error:
        if _directory_sync_is_unsupported(error):
            return
        raise
    try:
        try:
            os.fsync(descriptor)
        except OSError as error:
            if not _directory_sync_is_unsupported(error):
                raise
    finally:
        os.close(descriptor)


def fsync_tree(root: str | Path) -> None:
    """Flush every payload and directory in a private completed tree.

    Symlinks are refused: a generation must be immutable and self-contained,
    and following a link here would flush neither of those guarantees.
    """
    root = Path(root)
    entries = list(root.rglob("*"))
    links = [entry for entry in entries if entry.is_symlink()]
    if links:
        raise ValueError(f"published tree may not contain symlinks: {links[0]}")
    for entry in entries:
        if entry.is_file():
            fsync_file(entry)
    directories = [entry for entry in entries if entry.is_dir()]
    for directory in sorted(directories, key=lambda item: len(item.parts), reverse=True):
        fsync_directory(directory)
    fsync_directory(root)
