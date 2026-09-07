"""Preparation must never execute unverified cached source or accept wrong downloads."""

import hashlib
from unittest.mock import patch

import export_yamnet as export
import pytest


def test_cached_source_is_verified_before_use(tmp_path):
    (tmp_path / "yamnet.py").write_text("raise RuntimeError('untrusted')")
    with patch.object(export.urllib.request, "urlopen") as network:
        with pytest.raises(ValueError, match="Cached checksum mismatch"):
            export.fetch(tmp_path)
        network.assert_not_called()


def test_verified_cache_needs_no_network(tmp_path):
    data = b"verified fixture"
    (tmp_path / "fixture").write_bytes(data)
    with (
        patch.object(export, "HASHES", {"fixture": hashlib.sha256(data).hexdigest()}),
        patch.object(export.urllib.request, "urlopen") as network,
    ):
        export.fetch(tmp_path)
        network.assert_not_called()


def test_bad_download_is_not_saved(tmp_path):
    with patch.object(export.urllib.request, "urlopen") as network:
        network.return_value.__enter__.return_value.read.return_value = b"wrong"
        with pytest.raises(ValueError, match="Download checksum mismatch"):
            export.fetch(tmp_path)
        assert not (tmp_path / "yamnet.py").exists()
