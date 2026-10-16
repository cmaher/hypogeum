"""Behavioral tests using isolated, disposable Git repositories."""

import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest


SOURCE = "hypogeum"
SCRIPT = Path(__file__).resolve().parents[1] / "rename-project.py"
TARGET = "sample_app" if SOURCE != "sample_app" else "other_app"


def display(name):
    return "".join(part.capitalize() for part in name.split("_"))


class RenameProjectTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "--quiet")
        self.script = self.root / "scripts/rename-project.py"
        self.script.parent.mkdir()
        # A missing feature must fail as an assertion before the script exists.
        self.assertTrue(SCRIPT.is_file(), "project rename script has not been implemented")
        shutil.copy2(SCRIPT, self.script)
        self.fixture = self.root / "example.txt"
        self.fixture.write_text(
            f"{SOURCE} {SOURCE.upper()} {display(SOURCE)}\n"
            f"{SOURCE}_db {SOURCE}-db {SOURCE}.users.v1 {SOURCE.upper()}_LOG\n"
        )
        self.git("add", ".")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args])

    def run_script(self, *args, success=True, cwd=None):
        result = subprocess.run(
            [sys.executable, str(self.script), *args, "--output", "json"],
            cwd=cwd or self.root,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode == 0, success, result.stderr or result.stdout)
        stream = result.stdout if success else result.stderr
        return json.loads(stream)

    def snapshot(self):
        return {p: p.read_bytes() for p in (self.fixture, self.script)}

    def test_preview_is_complete_and_has_no_writes(self):
        before = self.snapshot()
        result = self.run_script(TARGET, "--dry-run", cwd=self.root.parent)
        self.assertTrue(result["ok"])
        self.assertTrue(result["data"]["dry_run"])
        paths = [entry["path"] for entry in result["data"]["files"]]
        self.assertEqual(paths, sorted(paths))
        self.assertIn("example.txt", paths)
        self.assertIn("scripts/rename-project.py", paths)
        self.assertGreater(result["data"]["replacements"], 6)
        self.assertEqual(self.snapshot(), before)

    def test_apply_replaces_forms_and_preserves_bytes_and_permissions(self):
        self.fixture.write_bytes(self.fixture.read_bytes().replace(b"\n", b"\r\n"))
        self.fixture.chmod(0o755)
        self.run_script(TARGET)
        self.assertEqual(
            self.fixture.read_bytes(),
            (f"{TARGET} {TARGET.upper()} {display(TARGET)}\r\n"
             f"{TARGET}_db {TARGET}-db {TARGET}.users.v1 {TARGET.upper()}_LOG\r\n").encode(),
        )
        self.assertEqual(stat.S_IMODE(self.fixture.stat().st_mode), 0o755)
        self.assertNotIn(SOURCE.encode(), self.script.read_bytes().lower())
        # The customized script can customize again, including PascalCase forms.
        self.run_script("next_project")
        self.assertIn("NextProject", self.fixture.read_text())
        self.assertNotIn(TARGET, self.fixture.read_text())

    def test_hidden_tracked_files_are_included_but_local_files_are_excluded(self):
        hidden = self.root / ".settings"
        hidden.write_text(SOURCE)
        self.git("add", ".settings")
        ignored = self.root / "local.env"
        ignored.write_text(SOURCE)
        (self.root / ".gitignore").write_text("local.env\n")
        untracked = self.root / "untracked.txt"
        untracked.write_text(SOURCE)
        self.run_script(TARGET)
        self.assertEqual(hidden.read_text(), TARGET)
        self.assertEqual(ignored.read_text(), SOURCE)
        self.assertEqual(untracked.read_text(), SOURCE)

    def test_invalid_names_fail_without_writing(self):
        names = ["", "a", "a1", "Upper", "has-dash", "has space", "../escape", "a__b", "a_", "1app",
                 "x\nname", "café", "select", "user", "self", "mod", "package", "apalis", "infra", "pg_custom", "a" * 64,
                 SOURCE, SOURCE + "_copy"]
        before = self.snapshot()
        for name in names:
            with self.subTest(name=name):
                result = self.run_script(name, success=False)
                self.assertFalse(result["ok"])
                self.assertEqual(result["error"]["code"], "invalid_name")
                self.assertEqual(self.snapshot(), before)

    def test_postgresql_system_user_is_rejected(self):
        before = self.snapshot()
        result = self.run_script("system_user", success=False)
        self.assertEqual(result["error"]["code"], "invalid_name")
        self.assertEqual(self.snapshot(), before)

    def test_identifier_length_accounts_for_existing_suffixes(self):
        self.fixture.write_text(SOURCE + "_long_existing_suffix")
        before = self.snapshot()
        result = self.run_script("a" * 50, success=False)
        self.assertEqual(result["error"]["code"], "identifier_too_long")
        self.assertEqual(self.snapshot(), before)

    def test_environment_names_are_not_database_identifiers(self):
        self.fixture.write_text(SOURCE + "_migrator " + SOURCE.upper() + "_MIGRATION_DATABASE_URL")
        self.run_script("a" * 50)
        self.assertIn("A" * 50 + "_MIGRATION_DATABASE_URL", self.fixture.read_text())

    def test_long_rust_test_names_are_not_database_identifiers(self):
        self.fixture.write_text("fn test_config_missing_url_error_names_" + SOURCE + "_migration_database_url() {}")
        self.run_script(TARGET)
        self.assertIn(TARGET, self.fixture.read_text())

    def test_unsupported_case_fails_before_any_write(self):
        self.fixture.write_text(SOURCE.swapcase()[:-1] + SOURCE[-1])
        before = self.snapshot()
        result = self.run_script(TARGET, success=False)
        self.assertEqual(result["error"]["code"], "unsupported_case")
        self.assertEqual(self.snapshot(), before)

    def test_matched_binary_is_rejected_and_unmatched_binary_is_skipped(self):
        binary = self.root / "image.bin"
        self.git("add", ".")
        for content in (b"\x00" + SOURCE.encode(), b"\xff" + SOURCE.encode()):
            binary.write_bytes(content)
            self.git("add", "image.bin")
            before = self.snapshot()
            result = self.run_script(TARGET, success=False)
            self.assertEqual(result["error"]["code"], "unsupported_file")
            self.assertEqual(self.snapshot(), before)
        binary.write_bytes(b"\x00\xffunchanged")
        self.run_script(TARGET)
        self.assertEqual(binary.read_bytes(), b"\x00\xffunchanged")

    def test_symlinks_are_never_followed(self):
        outside = self.root.parent / (self.root.name + "-outside")
        outside.write_text(SOURCE)
        self.addCleanup(outside.unlink)
        link = self.root / "link"
        link.symlink_to(outside)
        self.git("add", "link")
        self.run_script(TARGET)
        self.assertEqual(outside.read_text(), SOURCE)
        self.assertTrue(link.is_symlink())

    def test_matched_symlink_target_is_rejected(self):
        link = self.root / "link"
        link.symlink_to(SOURCE + "-missing")
        self.git("add", "link")
        before = self.snapshot()
        result = self.run_script(TARGET, success=False)
        self.assertEqual(result["error"]["code"], "unsupported_file")
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(os.readlink(link), SOURCE + "-missing")

    def test_symlink_parent_cannot_escape_checkout(self):
        folder = self.root / "nested"
        folder.mkdir()
        (folder / "tracked.txt").write_text(SOURCE)
        self.git("add", "nested")
        shutil.rmtree(folder)
        with tempfile.TemporaryDirectory() as external:
            outside = Path(external) / "tracked.txt"
            outside.write_text(SOURCE)
            folder.symlink_to(external, target_is_directory=True)
            before = self.snapshot()
            self.run_script(TARGET, success=False)
            self.assertEqual(self.snapshot(), before)
            self.assertEqual(outside.read_text(), SOURCE)

    def test_missing_tracked_file_is_preflight_error(self):
        self.fixture.unlink()
        before = self.script.read_bytes()
        self.run_script(TARGET, success=False)
        self.assertEqual(self.script.read_bytes(), before)

    def test_git_index_symlink_cannot_be_replaced_with_regular_file(self):
        link = self.root / "link"
        link.symlink_to("elsewhere")
        self.git("add", "link")
        link.unlink()
        link.write_text(SOURCE)
        before = self.snapshot()
        self.run_script(TARGET, success=False)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(link.read_text(), SOURCE)

    def test_describe_and_invalid_arguments_return_json(self):
        result = self.run_script("--describe")
        self.assertEqual(result["data"]["source"], SOURCE)
        self.assertIn("--dry-run", result["data"]["options"])
        result = self.run_script(TARGET, "--unknown", success=False)
        self.assertEqual(result["error"]["code"], "invalid_arguments")


if __name__ == "__main__":
    unittest.main()
