#!/usr/bin/env python3
"""Customize the tracked source of a fresh project before provisioning databases."""

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys


SOURCE = "hypogeum"
DISPLAY = "Hypogeum"
NAME_PATTERN = r"[a-z][a-z0-9]*(?:_[a-z0-9]+)*"
# PostgreSQL reserved keywords, Rust strict/reserved keywords, and protobuf
# declaration/scalar keywords. Keep this local: customization needs no network.
# PostgreSQL 18: https://www.postgresql.org/docs/18/sql-keywords-appendix.html
RESERVED = set("""
all analyse analyze and any array as asc asymmetric authorization binary both
case cast check collate collation column concurrently constraint create cross
current_catalog current_date current_role current_schema current_time
current_timestamp current_user default deferrable desc distinct do else end
except false fetch for foreign freeze from full grant group having ilike in
initially inner intersect into is isnull join lateral leading left like limit
localtime localtimestamp natural not notnull null offset on only or order outer
overlaps placing primary references returning right select session_user similar
some symmetric system_user table tablesample then to trailing true union unique user using
variadic verbose when where window with
abstract async await become box break const continue crate dyn enum extern final
fn gen if impl let loop macro match mod move mut override priv pub ref return
self static struct super trait try type typeof unsafe unsized use virtual where
yield
syntax import weak public package option optional required repeated oneof map
reserved extensions extend message service rpc returns stream enum double float
int32 int64 uint32 uint64 sint32 sint64 fixed32 fixed64 sfixed32 sfixed64 bool
string bytes group max true false edition
infra apalis public pg_catalog pg_temp
""".split())


class RenameError(Exception):
    def __init__(self, code, message):
        self.code = code
        super().__init__(message)


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise RenameError("invalid_arguments", message)


@dataclass
class Change:
    path: Path
    relative: str
    content: bytes
    replacements: int


@dataclass(frozen=True)
class NamingForms:
    lowercase: str
    uppercase: str
    pascal_case: str


def naming_forms(name):
    return NamingForms(
        lowercase=name,
        uppercase=name.upper(),
        pascal_case="".join(part.capitalize() for part in name.split("_")),
    )


def validate_name(name):
    if not re.fullmatch(NAME_PATTERN, name, flags=re.ASCII):
        raise RenameError(
            "invalid_name", "Use lowercase ASCII words separated by single underscores."
        )
    forms = naming_forms(name)
    if (
        name in RESERVED
        or name.startswith("pg_")
        or len(name) > 63
        or any(form.casefold() in name for form in (SOURCE, DISPLAY))
        or len({forms.lowercase, forms.uppercase, forms.pascal_case}) != 3
    ):
        raise RenameError(
            "invalid_name",
            "Name is reserved, too long, overlaps the current identity, "
            "or does not produce three distinct naming forms.",
        )


def git(root, *args):
    result = subprocess.run(["git", "-C", str(root), *args], capture_output=True)
    if result.returncode:
        raise RenameError("git_error", "Cannot read the Git index; run from a complete Git checkout.")
    return result.stdout


def tracked_files(root):
    entries = git(root, "ls-files", "--stage", "-z").split(b"\0")
    seen = set()
    files = []
    for entry in entries:
        if not entry:
            continue
        metadata, raw_path = entry.split(b"\t", 1)
        mode, _, stage = metadata.split()
        relative = os.fsdecode(raw_path)
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts or ".git" in path.parts:
            raise RenameError("unsupported_file", "Unsafe path in Git index.")
        if stage != b"0" or relative in seen:
            raise RenameError("unsupported_file", "Resolve Git index conflicts before renaming.")
        if mode not in (b"100644", b"100755", b"120000"):
            raise RenameError("unsupported_file", f"Unsupported Git entry: {relative}")
        seen.add(relative)
        files.append((relative, mode))
    return sorted(files)


def plan_changes(root, name):
    source = NamingForms(lowercase=SOURCE, uppercase=SOURCE.upper(), pascal_case=DISPLAY)
    target = naming_forms(name)
    replacements = {
        source.lowercase: target.lowercase,
        source.uppercase: target.uppercase,
        source.pascal_case: target.pascal_case,
    }
    pattern = re.compile(
        "|".join(re.escape(form) for form in sorted(replacements, key=len, reverse=True)),
        re.IGNORECASE | re.ASCII,
    )
    binary_pattern = re.compile(pattern.pattern.encode("ascii"), re.IGNORECASE | re.ASCII)
    changes = []
    for relative, index_mode in tracked_files(root):
        path = root / relative
        # lstat each parent: resolving a path first could follow an external link.
        for parent in reversed(Path(relative).parents):
            if (root / parent).is_symlink():
                raise RenameError("unsupported_file", f"Symlink directory in tracked path: {relative}")
        mode = path.lstat().st_mode
        if index_mode == b"120000" or stat.S_ISLNK(mode):
            if index_mode != b"120000" or not stat.S_ISLNK(mode):
                raise RenameError("unsupported_file", f"Tracked file type changed: {relative}")
            if binary_pattern.search(os.fsencode(os.readlink(path))):
                raise RenameError("unsupported_file", f"Project name occurs in symlink: {relative}")
            continue
        if not stat.S_ISREG(mode):
            raise RenameError("unsupported_file", f"Not a regular file: {relative}")
        content = path.read_bytes()
        if not binary_pattern.search(content):
            continue
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError:
            raise RenameError("unsupported_file", f"Project name occurs in non-UTF-8 file: {relative}") from None
        if "\0" in text:
            raise RenameError("unsupported_file", f"Project name occurs in binary file: {relative}")

        def replace(match):
            spelling = match.group()
            if spelling not in replacements:
                raise RenameError("unsupported_case", f"Unsupported project-name casing in: {relative}")
            return replacements[spelling]

        updated, count = pattern.subn(replace, text)
        # Project database identifiers start with the project stem. Rust test
        # functions may embed that stem in longer names without SQL's limit.
        for identifier in re.findall(r"[A-Za-z_][A-Za-z_0-9]*", text):
            if identifier.startswith(SOURCE):
                renamed = pattern.sub(replace, identifier)
                if len(renamed.encode("utf-8")) > 63:
                    raise RenameError(
                        "identifier_too_long",
                        f"Resulting identifier exceeds 63 bytes in: {relative}",
                    )
        changes.append(Change(path, relative, updated.encode("utf-8"), count))
    return changes


def describe():
    return {
        "source": SOURCE,
        "name_pattern": NAME_PATTERN,
        "forms": ["lowercase", "UPPERCASE", "PascalCase"],
        "options": ["--dry-run", "--output", "--describe", "--help"],
        "scope": "Git-tracked UTF-8 file contents; excludes untracked/ignored files and symlink targets",
        "output": {
            "ok": "boolean",
            "data": "dry_run, files[{path,replacements}], replacements",
            "error": "code, message",
        },
        "exit_codes": {"0": "success", "1": "invalid input, preflight or write failure"},
    }


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    # Determine error formatting even when argparse rejects the remaining input.
    json_output = any(
        arg == "--output=json"
        or (arg == "--output" and i + 1 < len(args) and args[i + 1] == "json")
        for i, arg in enumerate(args)
    )
    try:
        parser = Parser(description=__doc__)
        parser.add_argument("name", nargs="?", help="Lowercase project identifier, e.g. acme_tools")
        parser.add_argument("--dry-run", action="store_true", help="Preview all replacements without writing")
        parser.add_argument("--output", choices=["text", "json"], default="text")
        parser.add_argument("--describe", action="store_true", help="Describe the command as JSON")
        options = parser.parse_args(args)
        if options.describe:
            print(json.dumps({"ok": True, "data": describe()}, sort_keys=True))
            return 0
        if options.name is None:
            parser.error("a project name is required")
        validate_name(options.name)
        root = Path(__file__).resolve().parent.parent
        git_root = Path(os.fsdecode(git(root, "rev-parse", "--show-toplevel")).rstrip("\n"))
        if git_root.resolve() != root:
            raise RenameError("git_error", "Script must be in the scripts directory at the Git checkout root.")
        changes = plan_changes(root, options.name)
        if not options.dry_run:
            for change in changes:
                try:
                    change.path.write_bytes(change.content)
                except OSError as error:
                    raise RenameError(
                        "write_error",
                        f"Could not write {change.relative}; earlier files may have "
                        f"changed ({error.strerror}).",
                    ) from error
        data = {
            "dry_run": options.dry_run,
            "files": [{"path": c.relative, "replacements": c.replacements} for c in changes],
            "replacements": sum(c.replacements for c in changes),
        }
        if json_output:
            print(json.dumps({"ok": True, "data": data}, sort_keys=True))
        else:
            for entry in data["files"]:
                print(f"{entry['path']}: {entry['replacements']} replacements")
            action = "Would replace" if options.dry_run else "Replaced"
            print(f"{action} {data['replacements']} occurrences in {len(changes)} files.")
        return 0
    except (RenameError, OSError) as error:
        code = error.code if isinstance(error, RenameError) else "io_error"
        message = (
            str(error) if isinstance(error, RenameError)
            else f"Cannot inspect tracked files ({error.strerror})."
        )
        if json_output:
            print(json.dumps({"ok": False, "error": {"code": code, "message": message}}, sort_keys=True), file=sys.stderr)
        else:
            print(f"{code}: {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
