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


SOURCE = "exemplum"
SHORT = "exmpl"
SCRIPT = Path(__file__).resolve().parents[1] / "rename-project.py"
TARGET = "sample_app" if SOURCE != "sample_app" else "other_app"


def display(name):
    return "".join(part.capitalize() for part in name.split("_"))


class RenameProjectTests(unittest.TestCase):
    def setUp(self):
        target_dir = SCRIPT.parents[1] / "target/tmp"
        target_dir.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=target_dir)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "--quiet")
        self.script = self.root / "scripts/rename-project.py"
        self.script.parent.mkdir()
        # A missing feature must fail as an assertion before the script exists.
        self.assertTrue(SCRIPT.is_file(), "project rename script has not been implemented")
        shutil.copy2(SCRIPT, self.script)
        self.fixture = self.root / "example/fixture.txt"
        self.fixture.parent.mkdir(parents=True, exist_ok=True)
        self.fixture.write_text(
            f"{SOURCE} {SOURCE.upper()} {display(SOURCE)}\n"
            f"{SOURCE}_db {SOURCE}-db {SOURCE}.users.v1 {SOURCE.upper()}_LOG\n"
            f"{SHORT}-errors {SHORT}_ext_db {SHORT}_dmn_users\n"
        )
        self.root_file = self.root / "root.txt"
        self.root_file.write_text(f"{SOURCE} {SOURCE.upper()} {display(SOURCE)}\n")
        self.git("add", ".")

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root)

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
        return {p: p.read_bytes() for p in (self.fixture, self.script, self.root_file)}

    def test_preview_is_complete_and_has_no_writes(self):
        before = self.snapshot()
        result = self.run_script(TARGET, "--dry-run", cwd=self.root.parent)
        self.assertTrue(result["ok"])
        self.assertTrue(result["data"]["dry_run"])
        paths = [entry["path"] for entry in result["data"]["files"]]
        self.assertEqual(paths, sorted(paths))
        self.assertIn("example/fixture.txt", paths)
        self.assertNotIn("root.txt", paths)
        self.assertNotIn("scripts/rename-project.py", paths)
        self.assertGreater(result["data"]["replacements"], 6)
        self.assertEqual(self.snapshot(), before)

    def test_apply_replaces_forms_and_preserves_bytes_and_permissions(self):
        self.fixture.write_bytes(self.fixture.read_bytes().replace(b"\n", b"\r\n"))
        self.fixture.chmod(0o755)
        self.run_script(TARGET)
        self.assertEqual(
            self.fixture.read_bytes(),
            (f"{TARGET} {TARGET.upper()} {display(TARGET)}\r\n"
             f"{TARGET}_db {TARGET}-db {TARGET}.users.v1 {TARGET.upper()}_LOG\r\n"
             f"{TARGET}-errors {TARGET}_ext_db {TARGET}_dmn_users\r\n").encode(),
        )
        self.assertEqual(stat.S_IMODE(self.fixture.stat().st_mode), 0o755)
        # Root files and script outside example/ are never modified
        self.assertEqual(self.root_file.read_text(), f"{SOURCE} {SOURCE.upper()} {display(SOURCE)}\n")
        self.assertIn(SOURCE.encode(), self.script.read_bytes().lower())

    def test_app_and_rpc_configuration_stay_aligned(self):
        repository = SCRIPT.parent.parent
        paths = [
            "example/Cargo.toml",
            "example/docker-compose.yml",
            "example/cmd/app/src/main.rs",
            "example/cmd/db/src/main.rs",
            "example/crates/app/Cargo.toml",
            "example/crates/domain/users/src/repo/entity.rs",
            "example/crates/domain/users/src/repo/postgres.rs",
            "example/crates/domain/users/src/pg/migrations/m20260101_000001_create_users.rs",
            "example/crates/domain/users/src/pg/migrations/m20260101_000002_create_auth_identities.rs",
            "example/crates/domain/auth/Cargo.toml",
            "example/crates/domain/auth/src/lib.rs",
            "example/crates/domain/auth/src/models.rs",
            "example/crates/domain/auth/src/backend.rs",
            "example/crates/domain/auth/src/controller.rs",
            "example/crates/domain/auth/src/repo/mod.rs",
            "example/crates/domain/auth/src/repo/postgres.rs",
            "example/crates/domain/auth/src/session.rs",
            "example/crates/app/src/state.rs",
            "example/crates/app/src/router.rs",
            "example/infra/postgres/provision.sql",
            "example/config/schema.cedarschema",
            "example/config/policies.cedar",
        ]
        root_paths = [
            "Cargo.toml",
            "crates/core/rpc/src/lib.rs",
            "crates/ext/rdbms/src/lib.rs",
            "crates/core/envconfig/src/lib.rs",
        ]
        for relative in paths + root_paths:
            source = repository / relative
            self.assertTrue(source.is_file(), f"missing surface: {relative}")
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        self.git("add", ".")
        self.run_script(TARGET)

        compose = (self.root / "example/docker-compose.yml").read_text()
        app_main = (self.root / "example/cmd/app/src/main.rs").read_text()
        db_main = (self.root / "example/cmd/db/src/main.rs").read_text()
        app_cargo = (self.root / "example/crates/app/Cargo.toml").read_text()
        entity = (self.root / "example/crates/domain/users/src/repo/entity.rs").read_text()
        postgres = (self.root / "example/crates/domain/users/src/repo/postgres.rs").read_text()
        migration = (self.root / "example/crates/domain/users/src/pg/migrations/m20260101_000001_create_users.rs").read_text()
        migration_auth = (self.root / "example/crates/domain/users/src/pg/migrations/m20260101_000002_create_auth_identities.rs").read_text()
        auth_cargo = (self.root / "example/crates/domain/auth/Cargo.toml").read_text()
        auth_backend = (self.root / "example/crates/domain/auth/src/backend.rs").read_text()
        auth_models = (self.root / "example/crates/domain/auth/src/models.rs").read_text()
        auth_session = (self.root / "example/crates/domain/auth/src/session.rs").read_text()
        app_state = (self.root / "example/crates/app/src/state.rs").read_text()
        provision = (self.root / "example/infra/postgres/provision.sql").read_text()

        self.assertIn(f"{TARGET}-app", app_cargo)
        self.assertIn(f"{TARGET}-dmn-auth", auth_cargo)
        self.assertIn(f"pub struct {display(TARGET)}AuthBackend", auth_backend)
        self.assertIn(f'schema_name = "{TARGET}"', auth_models)
        self.assertIn(f'new_with_schema(pool, "{TARGET}")', auth_session)
        self.assertIn(f'new_with_schema(pool.clone(), "{TARGET}")', app_state)
        self.assertIn(f'GRANT SELECT, INSERT, UPDATE, DELETE ON {TARGET}.auth_identities TO {TARGET}_users', migration_auth)
        self.assertIn(f'GRANT SELECT, INSERT, UPDATE, DELETE ON {TARGET}.sessions TO {TARGET}_users', migration_auth)
        self.assertIn(f"POSTGRES_DB: {TARGET}", compose)
        self.assertIn(f"{TARGET.upper()}_POSTGRES_PASSWORD", compose)
        self.assertIn(f'"{TARGET.upper()}_APP_"', app_main)
        self.assertIn(f'"{TARGET.upper()}_MIGRATION_"', db_main)
        self.assertIn(f"Usage: {TARGET}-db", db_main)
        self.assertIn(f'schema_name = "{TARGET}"', entity)
        self.assertIn(f'role != "{TARGET}_users"', postgres)
        self.assertIn(f'SELECT {TARGET}.schema_version()', postgres)
        self.assertIn(f'CREATE SCHEMA IF NOT EXISTS {TARGET} AUTHORIZATION {TARGET}_owner', migration)
        self.assertIn(f'GRANT USAGE ON SCHEMA {TARGET} TO {TARGET}_users', migration)
        self.assertIn(f"{TARGET}_owner", provision)
        self.assertIn(f"{TARGET}_migrator", provision)
        self.assertIn(f"{TARGET}_users", provision)
        self.assertIn(f"{TARGET}_tasks", provision)

        for relative in paths:
            self.assertNotIn(SOURCE, (self.root / relative).read_text().lower())

        for relative in root_paths:
            self.assertEqual(
                (self.root / relative).read_bytes(),
                (repository / relative).read_bytes(),
                f"Root file modified: {relative}"
            )

    def test_hidden_tracked_files_are_included_but_local_files_are_excluded(self):
        hidden = self.root / "example/.settings"
        hidden.write_text(SOURCE)
        self.git("add", "example/.settings")
        ignored = self.root / "example/local.env"
        ignored.write_text(SOURCE)
        (self.root / "example/.gitignore").write_text("local.env\n")
        untracked = self.root / "example/untracked.txt"
        untracked.write_text(SOURCE)
        self.run_script(TARGET)
        self.assertEqual(hidden.read_text(), TARGET)
        self.assertEqual(ignored.read_text(), SOURCE)
        self.assertEqual(untracked.read_text(), SOURCE)
        self.assertEqual(self.root_file.read_text(), f"{SOURCE} {SOURCE.upper()} {display(SOURCE)}\n")

    def test_root_files_outside_example_remain_unchanged(self):
        root_sample = self.root / "sample_outside.txt"
        root_sample.write_text(f"{SOURCE} {SOURCE.upper()} {display(SOURCE)} {SHORT}\n")
        self.git("add", "sample_outside.txt")
        self.run_script(TARGET)
        self.assertEqual(root_sample.read_text(), f"{SOURCE} {SOURCE.upper()} {display(SOURCE)} {SHORT}\n")

    def test_invalid_names_fail_without_writing(self):
        names = ["", "a", "a1", "Upper", "has-dash", "has space", "../escape", "a__b", "a_", "1app",
                 "x\nname", "café", "select", "user", "self", "mod", "package", "apalis", "infra", "pg_custom", "a" * 64,
                 SOURCE, SOURCE + "_copy", SHORT, SHORT + "_copy"]
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
        binary = self.root / "example/image.bin"
        self.git("add", ".")
        for content in (b"\x00" + SOURCE.encode(), b"\xff" + SOURCE.encode()):
            binary.write_bytes(content)
            self.git("add", "example/image.bin")
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
        link = self.root / "example/link"
        link.symlink_to(f"../../{outside.name}")
        self.git("add", "example/link")
        self.run_script(TARGET)
        self.assertEqual(outside.read_text(), SOURCE)
        self.assertTrue(link.is_symlink())

    def test_matched_symlink_target_is_rejected(self):
        link = self.root / "example/link"
        link.symlink_to(SOURCE + "-missing")
        self.git("add", "example/link")
        before = self.snapshot()
        result = self.run_script(TARGET, success=False)
        self.assertEqual(result["error"]["code"], "unsupported_file")
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(os.readlink(link), SOURCE + "-missing")

    def test_symlink_parent_cannot_escape_checkout(self):
        folder = self.root / "example/nested"
        folder.mkdir()
        (folder / "tracked.txt").write_text(SOURCE)
        self.git("add", "example/nested")
        shutil.rmtree(folder)
        with tempfile.TemporaryDirectory() as external:
            outside = Path(external) / "tracked.txt"
            outside.write_text(SOURCE)
            folder.symlink_to(external, target_is_directory=True)
            before = self.snapshot()
            result = self.run_script(TARGET, success=False)
            self.assertEqual(self.snapshot(), before)
            self.assertEqual(outside.read_text(), SOURCE)

    def test_missing_tracked_file_is_preflight_error(self):
        self.fixture.unlink()
        before = self.script.read_bytes()
        self.run_script(TARGET, success=False)
        self.assertEqual(self.script.read_bytes(), before)

    def test_git_index_symlink_cannot_be_replaced_with_regular_file(self):
        link = self.root / "example/link"
        link.symlink_to("elsewhere")
        self.git("add", "example/link")
        link.unlink()
        link.write_text(SOURCE)
        before = self.snapshot()
        self.run_script(TARGET, success=False)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(link.read_text(), SOURCE)

    def test_describe_and_invalid_arguments_return_json(self):
        result = self.run_script("--describe")
        self.assertEqual(result["data"]["source"], SOURCE)
        self.assertEqual(result["data"]["short"], SHORT)
        self.assertIn("--dry-run", result["data"]["options"])
        result = self.run_script(TARGET, "--unknown", success=False)
        self.assertEqual(result["error"]["code"], "invalid_arguments")


if __name__ == "__main__":
    unittest.main()
