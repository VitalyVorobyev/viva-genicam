"""Focused regressions for the release archive boundary (no camera needed)."""
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("distribution", Path(__file__).with_name("check-distribution.py"))
distribution = importlib.util.module_from_spec(spec)
spec.loader.exec_module(distribution)
notice_spec = importlib.util.spec_from_file_location("notices", Path(__file__).with_name("dependency-notices.py"))
notices = importlib.util.module_from_spec(notice_spec)
notice_spec.loader.exec_module(notices)


class DistributionTests(unittest.TestCase):
    def test_real_archives_check_notice_contents_case_insensitively(self):
        for extension in ["zip", "tar.gz"]:
            for name in ["THIRD_PARTY_NOTICES.txt", "third_party_notices.txt"]:
                for content, valid in [(b"development placeholder", False),
                                       (b"Generated from locked, target-filtered dependency metadata", True)]:
                    with self.subTest(extension=extension, name=name, valid=valid), tempfile.TemporaryDirectory() as tmp:
                        path = Path(tmp) / f"artifact.{extension}"
                        if extension == "zip":
                            with zipfile.ZipFile(path, "w") as archive:
                                archive.writestr(name, content)
                        else:
                            with tarfile.open(path, "w:gz") as archive:
                                member = tarfile.TarInfo(name)
                                member.size = len(content)
                                archive.addfile(member, io.BytesIO(content))
                        if valid:
                            distribution.check_archive(path)
                        else:
                            with self.assertRaisesRegex(ValueError, "placeholder"):
                                distribution.check_archive(path)

    def test_real_zip_rejects_bench_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "artifact.whl"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("THIRD_PARTY_NOTICES.txt", "Generated from locked, target-filtered dependency metadata")
                archive.writestr("target/hardware/camera.xml", "private camera data")
            with self.assertRaisesRegex(ValueError, "development material"):
                distribution.check_archive(path)

    def test_tar_directory_cannot_substitute_for_notice(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "artifact.tar.gz"
            with tarfile.open(path, "w:gz") as archive:
                member = tarfile.TarInfo("THIRD_PARTY_NOTICES.txt")
                member.type = tarfile.DIRTYPE
                archive.addfile(member)
            with self.assertRaisesRegex(ValueError, "no regular"):
                distribution.check_archive(path)
    def test_valid_wheel(self):
        distribution.check_names(["viva_genicam/_native.so", "viva_genicam/THIRD_PARTY_NOTICES.txt"])

    def test_missing_notice(self):
        with self.assertRaisesRegex(ValueError, "no THIRD"):
            distribution.check_names(["viva-camctl"])

    def test_placeholder_is_not_a_release_notice(self):
        with self.assertRaisesRegex(ValueError, "placeholder"):
            distribution.check_notice(b"Development build: generate notices before distribution")
        distribution.check_notice(b"Generated from locked, target-filtered dependency metadata")

    def test_private_and_fetched_material(self):
        for name in ["tc40.pcap", "fixtures/vendor-xml/camera.xml", ".local-evidence/report.txt",
                     "target/gentl/emva/GenTL.h", "viva-report.txt", "../escape",
                     "target/legal/report.txt", "report-before.txt"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                distribution.check_names(["THIRD_PARTY_NOTICES.txt", name])


class NoticeTests(unittest.TestCase):
    def test_license_and_notice_files_are_retained(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "LICENSE-MIT").write_text("MIT text")
            (root / "NOTICE").write_text("attribution")
            (root / "src.rs").write_text("not license data")
            (root / "vendor").mkdir()
            (root / "vendor/LICENSE").write_text("bundled native library terms")
            (root / "vendor/license.rs").write_text("source code, not license text")
            self.assertEqual({name for name, _ in notices.texts(root)}, {"LICENSE-MIT", "NOTICE", "vendor/LICENSE"})

    def test_missing_license_is_not_silently_omitted(self):
        with tempfile.TemporaryDirectory() as tmp, self.assertRaisesRegex(ValueError, "No license text"):
            notices.texts(Path(tmp))

    def test_only_selected_roots_normal_and_build_edges(self):
        packages = [{"id": name, "name": name, "version": "1", "manifest_path": "/tmp/Cargo.toml",
                     "source": None, "license": "MIT"}
                    for name in ["viva-gentl", "normal", "build", "dev", "unrelated"]]
        edges = [{"pkg": name, "dep_kinds": [{"kind": kind}]}
                 for name, kind in [("normal", None), ("build", "build"), ("dev", "dev")]]
        metadata = {"packages": packages, "resolve": {"nodes": [
            {"id": p["id"], "deps": edges if p["id"] == "viva-gentl" else []} for p in packages]}}
        with patch.object(notices.subprocess, "check_output", return_value=json.dumps(metadata)), \
                patch.object(notices, "texts", return_value=[("LICENSE", "MIT text")]):
            inventory = notices.rust_inventory("gentl", "aarch64-apple-darwin")
        self.assertEqual({row[0] for row in inventory}, {"Rust: viva-gentl 1", "Rust: normal 1", "Rust: build 1"})


if __name__ == "__main__":
    unittest.main()
