#!/usr/bin/env python3
"""Assemble and verify reproducible Kunlun JavaScriptCore artifacts."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from typing import Any, Iterable


SUPPORTED_TARGETS = {
    "aarch64-apple-darwin": "arm64",
    "x86_64-apple-darwin": "x86_64",
}
ALLOWED_TOP_LEVEL = {"include", "lib", "licenses", "metadata"}


class ArtifactError(RuntimeError):
    """A fail-closed artifact validation error."""


def read_json(path: Path) -> dict[str, Any]:
    """Read a JSON object from *path*."""
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"could not read JSON object {path}: {error}") from error
    if not isinstance(value, dict):
        raise ArtifactError(f"expected a JSON object in {path}")
    return value


def write_json(path: Path, value: dict[str, Any]) -> None:
    """Write stable, reviewable JSON with a trailing newline."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n",
        encoding="utf-8",
    )


def sha256_file(path: Path) -> str:
    """Return the lowercase SHA-256 digest of a file."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def sha1_file(path: Path) -> str:
    """Return the lowercase SHA-1 digest required by SPDX verification codes."""
    digest = hashlib.sha1(usedforsecurity=False)
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_target(manifest: dict[str, Any], target: str) -> dict[str, Any]:
    """Return the unique target entry from the distribution manifest."""
    if target not in SUPPORTED_TARGETS:
        raise ArtifactError(f"unsupported macOS target: {target}")
    matches = [entry for entry in manifest.get("targets", []) if entry.get("triple") == target]
    if len(matches) != 1:
        raise ArtifactError(f"manifest must contain exactly one target entry for {target}")
    return matches[0]


def checked_relative_path(value: str, label: str) -> PurePosixPath:
    """Reject absolute paths and traversal before using manifest paths."""
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts:
        raise ArtifactError(f"{label} is not a safe relative path: {value!r}")
    return path


def license_filename(index: int, component: str, source_path: str) -> str:
    """Create a deterministic, collision-resistant license filename."""
    slug = "".join(character.lower() if character.isalnum() else "-" for character in component)
    slug = "-".join(part for part in slug.split("-") if part)
    basename = PurePosixPath(source_path).name
    return f"{index:02d}-{slug}-{basename}"


def copy_exact(source: Path, destination: Path, expected_digest: str | None = None) -> None:
    """Copy a regular file and optionally enforce its reviewed digest."""
    if not source.is_file() or source.is_symlink():
        raise ArtifactError(f"required regular file is missing: {source}")
    if expected_digest is not None:
        actual = sha256_file(source)
        if actual != expected_digest:
            raise ArtifactError(
                f"digest mismatch for {source}: expected {expected_digest}, computed {actual}"
            )
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def collect_tool_versions() -> dict[str, str]:
    """Capture the native tool versions that can affect a macOS artifact."""
    commands = {
        "xcode": ["xcodebuild", "-version"],
        "apple-clang": ["xcrun", "clang", "--version"],
        "macos-sdk": ["xcrun", "--sdk", "macosx", "--show-sdk-version"],
        "cmake": ["cmake", "--version"],
        "python": ["/usr/bin/python3", "--version"],
        "perl": ["/usr/bin/perl", "-e", "printf qq{%vd\\n}, $^V"],
        "ruby": ["/usr/bin/ruby", "--version"],
        "git": ["/usr/bin/git", "--version"],
    }
    versions: dict[str, str] = {}
    for name, command in commands.items():
        try:
            output = subprocess.run(
                command,
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            ).stdout.strip()
        except (OSError, subprocess.CalledProcessError) as error:
            raise ArtifactError(f"could not record {name} version: {error}") from error
        versions[name] = " | ".join(line.strip() for line in output.splitlines() if line.strip())
    return versions


def runner_metadata() -> dict[str, Any]:
    """Record CI runner identity without treating a moving label as a toolchain pin."""
    return {
        "github_actions": os.environ.get("GITHUB_ACTIONS") == "true",
        "image_os": os.environ.get("ImageOS"),
        "image_version": os.environ.get("ImageVersion"),
        "runner_arch": os.environ.get("RUNNER_ARCH"),
        "runner_name": os.environ.get("RUNNER_NAME"),
        "workflow_ref": os.environ.get("GITHUB_WORKFLOW_REF"),
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
    }


def create_build_metadata(
    manifest: dict[str, Any], target_entry: dict[str, Any], target: str
) -> dict[str, Any]:
    """Build the audit record embedded in every archive."""
    source_epoch = int(manifest["build"]["environment"]["SOURCE_DATE_EPOCH"])
    return {
        "schema_version": 1,
        "distribution": manifest["distribution"],
        "source": manifest["source"],
        "target": {
            "triple": target,
            "arch": target_entry["arch"],
            "deployment_target": target_entry["deployment_target"],
        },
        "build": manifest["build"],
        "toolchain_id": target_entry["toolchain"],
        "observed_tools": collect_tool_versions(),
        "runner": runner_metadata(),
        "abi": manifest["abi"],
        "artifact": {
            "archive_path": target_entry["artifact"]["archive_path"],
            "sbom": target_entry["artifact"]["sbom"],
            "provenance": target_entry["artifact"]["provenance"],
        },
        "created": dt.datetime.fromtimestamp(source_epoch, tz=dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
    }


def iter_regular_files(root: Path) -> Iterable[Path]:
    """Yield artifact files in stable archive order."""
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        if path.is_symlink():
            raise ArtifactError(f"artifact staging tree must not contain symlinks: {path}")
        if path.is_file():
            yield path
        elif not path.is_dir():
            raise ArtifactError(f"artifact staging tree contains a special file: {path}")


def spdx_id(relative_path: str) -> str:
    """Create a stable SPDX identifier for an artifact file."""
    suffix = hashlib.sha256(relative_path.encode("utf-8")).hexdigest()[:24]
    return f"SPDXRef-File-{suffix}"


def file_license(relative_path: str, manifest: dict[str, Any], licenses: dict[str, str]) -> str:
    """Return the reviewed license expression applicable to an archived file."""
    if relative_path == "include/kunlun_jsc.h":
        return "MIT"
    if relative_path.startswith("licenses/"):
        return licenses.get(PurePosixPath(relative_path).name, "NOASSERTION")
    if relative_path.startswith("lib/libkunlun_jsc"):
        return "MIT"
    if relative_path.startswith("lib/libJavaScriptCore"):
        expressions = sorted(
            {
                entry["spdx_expression"]
                for entry in manifest["licenses"]
                if entry["component"].startswith("WebKit")
            }
        )
        return " AND ".join(f"({expression})" for expression in expressions)
    return "NOASSERTION"


def generate_sbom(
    staging: Path, output: Path, manifest: dict[str, Any], target: str
) -> dict[str, Any]:
    """Generate an SPDX 2.3 JSON inventory for the exact archive contents."""
    source_epoch = int(manifest["build"]["environment"]["SOURCE_DATE_EPOCH"])
    created = (
        dt.datetime.fromtimestamp(source_epoch, tz=dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )
    license_map = {
        license_filename(index, entry["component"], entry["source"]["path"]): entry[
            "spdx_expression"
        ]
        for index, entry in enumerate(manifest["licenses"], start=1)
    }
    files = []
    verification_hashes = []
    relationships = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": "SPDXRef-Package-kunlun-jsc",
        }
    ]
    for path in iter_regular_files(staging):
        relative = path.relative_to(staging).as_posix()
        identifier = spdx_id(relative)
        sha256 = sha256_file(path)
        files.append(
            {
                "SPDXID": identifier,
                "fileName": f"./{relative}",
                "checksums": [
                    {"algorithm": "SHA1", "checksumValue": sha1_file(path)},
                    {"algorithm": "SHA256", "checksumValue": sha256},
                ],
                "licenseConcluded": file_license(relative, manifest, license_map),
                "copyrightText": "NOASSERTION",
            }
        )
        verification_hashes.append(sha1_file(path))
        relationships.append(
            {
                "spdxElementId": "SPDXRef-Package-kunlun-jsc",
                "relationshipType": "CONTAINS",
                "relatedSpdxElement": identifier,
            }
        )
    package_verification = hashlib.sha1(
        "".join(sorted(verification_hashes)).encode("ascii"), usedforsecurity=False
    ).hexdigest()
    declared = sorted({entry["spdx_expression"] for entry in manifest["licenses"]})
    sbom = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"kunlun-jsc-{target}",
        "documentNamespace": (
            "https://github.com/kunlunengine/runtime/spdx/"
            f"{manifest['source']['revision']}/{target}"
        ),
        "creationInfo": {
            "created": created,
            "creators": ["Organization: Kunlun Engine", "Tool: jsc_artifact.py"],
        },
        "packages": [
            {
                "name": "kunlun-jsc",
                "SPDXID": "SPDXRef-Package-kunlun-jsc",
                "versionInfo": manifest["source"]["revision"],
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": True,
                "packageVerificationCode": {
                    "packageVerificationCodeValue": package_verification
                },
                "licenseConcluded": " AND ".join(
                    f"({expression})" for expression in declared
                ),
                "licenseDeclared": " AND ".join(
                    f"({expression})" for expression in declared
                ),
                "copyrightText": "NOASSERTION",
            }
        ],
        "files": files,
        "relationships": relationships,
    }
    write_json(output, sbom)
    return sbom


def add_tar_directory(archive: tarfile.TarFile, relative: PurePosixPath, epoch: int) -> None:
    """Add a normalized directory entry to a deterministic tar archive."""
    info = tarfile.TarInfo(relative.as_posix())
    info.type = tarfile.DIRTYPE
    info.mode = 0o755
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = epoch
    archive.addfile(info)


def create_archive(staging: Path, output: Path, epoch: int, zstd: str) -> None:
    """Create a byte-stable ustar archive and single-threaded zstd stream."""
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kunlun-jsc-tar-") as temporary:
        tar_path = Path(temporary) / "artifact.tar"
        with tarfile.open(tar_path, "w", format=tarfile.USTAR_FORMAT) as archive:
            directories = sorted(
                {
                    parent
                    for file_path in iter_regular_files(staging)
                    for parent in PurePosixPath(file_path.relative_to(staging).as_posix()).parents
                    if parent != PurePosixPath(".")
                },
                key=lambda path: (len(path.parts), path.as_posix()),
            )
            for directory in directories:
                add_tar_directory(archive, directory, epoch)
            for path in iter_regular_files(staging):
                relative = path.relative_to(staging).as_posix()
                info = tarfile.TarInfo(relative)
                info.size = path.stat().st_size
                info.mode = 0o755 if relative.startswith("lib/") else 0o644
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mtime = epoch
                with path.open("rb") as source:
                    archive.addfile(info, source)
        try:
            subprocess.run(
                [zstd, "-q", "-19", "--threads=1", "--force", str(tar_path), "-o", str(output)],
                check=True,
            )
        except (OSError, subprocess.CalledProcessError) as error:
            raise ArtifactError(f"could not compress artifact with zstd: {error}") from error


def assemble(args: argparse.Namespace) -> None:
    """Assemble licenses, metadata, SBOM, and archive from built dylibs."""
    manifest = read_json(args.manifest)
    target_entry = manifest_target(manifest, args.target)
    expected_libraries = [PurePosixPath(path) for path in target_entry["artifact"]["library_paths"]]
    if expected_libraries != [
        PurePosixPath("lib/libJavaScriptCore.dylib"),
        PurePosixPath("lib/libkunlun_jsc.dylib"),
    ]:
        raise ArtifactError(f"unexpected macOS library layout for {args.target}")

    output_root = args.output.resolve()
    staging_root = args.staging.resolve()
    try:
        staging_root.relative_to(output_root)
    except ValueError as error:
        raise ArtifactError("staging directory must be contained by the output directory") from error
    if staging_root == output_root:
        raise ArtifactError("staging directory must not equal the output directory")
    args.staging = staging_root
    args.output = output_root
    if args.staging.exists():
        shutil.rmtree(args.staging)
    args.staging.mkdir(parents=True)
    repository_root = args.repository_root.resolve()
    webkit_root = args.webkit_root.resolve()
    public_headers = manifest["abi"]["public_headers"]
    if public_headers != ["include/kunlun_jsc.h"]:
        raise ArtifactError("macOS artifacts must expose only include/kunlun_jsc.h")
    copy_exact(
        repository_root / "crates/kunlun-jsc-sys/include/kunlun_jsc.h",
        args.staging / public_headers[0],
    )
    copy_exact(args.jsc_library, args.staging / expected_libraries[0].as_posix())
    copy_exact(args.shim_library, args.staging / expected_libraries[1].as_posix())

    packaged_licenses = []
    for index, entry in enumerate(manifest["licenses"], start=1):
        source = entry["source"]
        relative = checked_relative_path(source["path"], f"licenses[{index - 1}].source.path")
        root = repository_root if source["kind"] == "local" else webkit_root
        filename = license_filename(index, entry["component"], source["path"])
        copy_exact(root / relative.as_posix(), args.staging / "licenses" / filename, entry["sha256"])
        packaged_licenses.append(
            {
                "component": entry["component"],
                "spdx_expression": entry["spdx_expression"],
                "path": f"licenses/{filename}",
                "sha256": entry["sha256"],
            }
        )

    metadata = create_build_metadata(manifest, target_entry, args.target)
    metadata["licenses"] = packaged_licenses
    write_json(args.staging / "metadata/build.json", metadata)

    sbom_relative = checked_relative_path(
        target_entry["artifact"]["sbom"]["path"], "artifact.sbom.path"
    )
    sbom_path = args.output / sbom_relative.as_posix()
    generate_sbom(args.staging, sbom_path, manifest, args.target)

    archive_relative = checked_relative_path(
        target_entry["artifact"]["archive_path"], "artifact.archive_path"
    )
    archive_path = args.output / archive_relative.as_posix()
    epoch = int(manifest["build"]["environment"]["SOURCE_DATE_EPOCH"])
    create_archive(args.staging, archive_path, epoch, args.zstd)
    print(
        json.dumps(
            {
                "archive": str(archive_path),
                "archive_sha256": sha256_file(archive_path),
                "sbom": str(sbom_path),
                "sbom_sha256": sha256_file(sbom_path),
                "staging": str(args.staging),
            },
            sort_keys=True,
        )
    )


def decompress_archive(archive: Path, zstd: str, destination: Path) -> list[tarfile.TarInfo]:
    """Safely extract a zstd-compressed tar while rejecting archive tricks."""
    tar_path = destination / "artifact.tar"
    try:
        with tar_path.open("wb") as output:
            subprocess.run([zstd, "-q", "-d", "-c", str(archive)], check=True, stdout=output)
    except (OSError, subprocess.CalledProcessError) as error:
        raise ArtifactError(f"could not decompress {archive}: {error}") from error
    extract_root = destination / "root"
    extract_root.mkdir()
    with tarfile.open(tar_path, "r:") as source:
        members = source.getmembers()
        names: set[str] = set()
        for member in members:
            path = PurePosixPath(member.name)
            normalized = path.as_posix()
            if (
                not member.name
                or not path.parts
                or path.is_absolute()
                or ".." in path.parts
                or member.name != normalized
                or normalized in names
                or not (member.isfile() or member.isdir())
            ):
                raise ArtifactError(f"unsafe or duplicate archive member: {member.name!r}")
            names.add(normalized)
            if path.parts[0] not in ALLOWED_TOP_LEVEL:
                raise ArtifactError(f"unexpected top-level archive member: {member.name}")
        source.extractall(extract_root, members=members)
    return members


def verify_spdx(sbom: dict[str, Any], extract_root: Path, target: str) -> None:
    """Verify SPDX identity and every recorded file checksum."""
    if sbom.get("spdxVersion") != "SPDX-2.3":
        raise ArtifactError("SBOM is not SPDX-2.3")
    if sbom.get("name") != f"kunlun-jsc-{target}":
        raise ArtifactError("SBOM target identity does not match the archive")
    expected: dict[str, tuple[str, str]] = {}
    for entry in sbom.get("files", []):
        name = entry.get("fileName", "")
        if not isinstance(name, str) or not name.startswith("./"):
            raise ArtifactError(f"invalid SBOM file name: {name!r}")
        relative = checked_relative_path(name[2:], "SBOM fileName").as_posix()
        checksums = entry.get("checksums", [])
        values = {
            item.get("algorithm"): item.get("checksumValue")
            for item in checksums
            if isinstance(item, dict)
        }
        sha1 = values.get("SHA1")
        sha256 = values.get("SHA256")
        if not isinstance(sha1, str) or not re.fullmatch(r"[0-9a-f]{40}", sha1):
            raise ArtifactError(f"SBOM inventory mismatch: invalid SHA1 for {relative}")
        if not isinstance(sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", sha256):
            raise ArtifactError(f"SBOM inventory mismatch: invalid SHA256 for {relative}")
        if relative in expected:
            raise ArtifactError(f"duplicate SBOM file entry: {relative}")
        expected[relative] = (sha1, sha256)
    actual = {
        path.relative_to(extract_root).as_posix(): (sha1_file(path), sha256_file(path))
        for path in iter_regular_files(extract_root)
    }
    if expected != actual:
        missing = sorted(actual.keys() - expected.keys())
        extra = sorted(expected.keys() - actual.keys())
        mismatched = sorted(
            path for path in actual.keys() & expected.keys() if actual[path] != expected[path]
        )
        raise ArtifactError(
            f"SBOM inventory mismatch: missing={missing}, extra={extra}, mismatched={mismatched}"
        )


def command_output(command: list[str]) -> str:
    """Run a native inspection command and return trimmed standard output."""
    try:
        return subprocess.run(
            command,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise ArtifactError(f"inspection command failed ({' '.join(command)}): {error}") from error


def verify_macho(extract_root: Path, target: str, deployment_target: str) -> None:
    """Enforce architecture, install-name, dependency, and shim-symbol boundaries."""
    expected_arch = SUPPORTED_TARGETS[target]
    libraries = [
        extract_root / "lib/libJavaScriptCore.dylib",
        extract_root / "lib/libkunlun_jsc.dylib",
    ]
    for library in libraries:
        archs = command_output(["lipo", "-archs", str(library)]).split()
        if archs != [expected_arch]:
            raise ArtifactError(f"{library.name} architectures are {archs}, expected {expected_arch}")
        dependencies = command_output(["otool", "-L", str(library)]).splitlines()[1:]
        for dependency in dependencies:
            name = dependency.strip().split(" ", 1)[0]
            if not name.startswith(("@rpath/", "@loader_path/", "/System/Library/", "/usr/lib/")):
                raise ArtifactError(f"{library.name} has non-hermetic dependency {name}")
            if name.startswith(("@rpath/", "@loader_path/")) and PurePosixPath(name).name not in {
                "libJavaScriptCore.dylib",
                "libkunlun_jsc.dylib",
            }:
                raise ArtifactError(f"{library.name} references unpackaged dependency {name}")
        build_versions = re.findall(
            r"\bminos\s+([0-9]+(?:\.[0-9]+){1,2})",
            command_output(["otool", "-l", str(library)]),
        )
        if not build_versions or any(version != deployment_target for version in build_versions):
            raise ArtifactError(
                f"{library.name} deployment targets are {build_versions}, expected {deployment_target}"
            )
    jsc_id = command_output(["otool", "-D", str(libraries[0])]).splitlines()[-1].strip()
    shim_id = command_output(["otool", "-D", str(libraries[1])]).splitlines()[-1].strip()
    if jsc_id != "@rpath/libJavaScriptCore.dylib":
        raise ArtifactError(f"unexpected JavaScriptCore install name: {jsc_id}")
    if shim_id != "@rpath/libkunlun_jsc.dylib":
        raise ArtifactError(f"unexpected shim install name: {shim_id}")
    symbols = command_output(["nm", "-gjU", str(libraries[1])]).splitlines()
    if not symbols or any(not symbol.startswith("_kunlun_jsc_") for symbol in symbols):
        raise ArtifactError("shim exports symbols outside the kunlun_jsc_* allowlist")


def verify(args: argparse.Namespace) -> None:
    """Verify archive structure, reproducibility metadata, SBOM, and Mach-O files."""
    manifest = read_json(args.manifest)
    target_entry = manifest_target(manifest, args.target)
    expected_archive = PurePosixPath(target_entry["artifact"]["archive_path"]).name
    expected_sbom = PurePosixPath(target_entry["artifact"]["sbom"]["path"]).name
    if args.archive.name != expected_archive:
        raise ArtifactError(f"archive must be named {expected_archive}")
    if args.sbom.name != expected_sbom:
        raise ArtifactError(f"SBOM must be named {expected_sbom}")

    with tempfile.TemporaryDirectory(prefix="kunlun-jsc-verify-") as temporary:
        temporary_path = Path(temporary)
        members = decompress_archive(args.archive, args.zstd, temporary_path)
        extract_root = temporary_path / "root"
        epoch = int(manifest["build"]["environment"]["SOURCE_DATE_EPOCH"])
        for member in members:
            expected_mode = 0o755 if member.isdir() or member.name.startswith("lib/") else 0o644
            if member.uid != 0 or member.gid != 0 or member.mtime != epoch:
                raise ArtifactError(f"non-normalized archive metadata for {member.name}")
            if stat.S_IMODE(member.mode) != expected_mode:
                raise ArtifactError(f"unexpected archive mode for {member.name}")
        required = {
            "include/kunlun_jsc.h",
            "lib/libJavaScriptCore.dylib",
            "lib/libkunlun_jsc.dylib",
            "metadata/build.json",
        }
        actual_files = {
            path.relative_to(extract_root).as_posix() for path in iter_regular_files(extract_root)
        }
        missing = sorted(required - actual_files)
        if missing:
            raise ArtifactError(f"archive is missing required files: {missing}")
        metadata = read_json(extract_root / "metadata/build.json")
        expected_licenses = [
            {
                "component": entry["component"],
                "spdx_expression": entry["spdx_expression"],
                "path": (
                    "licenses/"
                    f"{license_filename(index, entry['component'], entry['source']['path'])}"
                ),
                "sha256": entry["sha256"],
            }
            for index, entry in enumerate(manifest["licenses"], start=1)
        ]
        expected_files = required | {entry["path"] for entry in expected_licenses}
        extra = sorted(actual_files - expected_files)
        missing = sorted(expected_files - actual_files)
        if missing or extra:
            raise ArtifactError(
                f"archive file inventory mismatch: missing={missing}, extra={extra}"
            )
        if metadata.get("distribution") != manifest["distribution"]:
            raise ArtifactError("archive distribution metadata does not match the manifest")
        if metadata.get("source") != manifest["source"]:
            raise ArtifactError("archive source metadata does not match the manifest")
        expected_target = {
            "triple": args.target,
            "arch": target_entry["arch"],
            "deployment_target": target_entry["deployment_target"],
        }
        if metadata.get("target") != expected_target:
            raise ArtifactError("archive target metadata does not match the requested target")
        if metadata.get("build") != manifest["build"]:
            raise ArtifactError("archive build metadata does not match the manifest")
        if metadata.get("toolchain_id") != target_entry["toolchain"]:
            raise ArtifactError("archive toolchain metadata does not match the manifest")
        if metadata.get("abi") != manifest["abi"]:
            raise ArtifactError("archive ABI metadata does not match the manifest")
        if metadata.get("licenses") != expected_licenses:
            raise ArtifactError("archive license metadata does not match the manifest")
        expected_artifact = {
            "archive_path": target_entry["artifact"]["archive_path"],
            "sbom": target_entry["artifact"]["sbom"],
            "provenance": target_entry["artifact"]["provenance"],
        }
        if metadata.get("artifact") != expected_artifact:
            raise ArtifactError("archive artifact metadata does not match the manifest")
        verify_spdx(read_json(args.sbom), extract_root, args.target)
        if not args.skip_macho:
            verify_macho(
                extract_root,
                args.target,
                target_entry["deployment_target"]["minimum"],
            )

    print(
        json.dumps(
            {
                "archive": str(args.archive),
                "archive_sha256": sha256_file(args.archive),
                "sbom": str(args.sbom),
                "sbom_sha256": sha256_file(args.sbom),
                "target": args.target,
                "verified": True,
            },
            sort_keys=True,
        )
    )


def archive_inventory(archive: Path, zstd: str) -> dict[str, str]:
    """Return member digests for rebuild comparison."""
    with tempfile.TemporaryDirectory(prefix="kunlun-jsc-compare-") as temporary:
        root = Path(temporary)
        decompress_archive(archive, zstd, root)
        return {
            path.relative_to(root / "root").as_posix(): sha256_file(path)
            for path in iter_regular_files(root / "root")
        }


def compare(args: argparse.Namespace) -> None:
    """Write a machine-readable comparison of two independent rebuilds."""
    first = archive_inventory(args.first, args.zstd)
    second = archive_inventory(args.second, args.zstd)
    differences = [
        {
            "path": path,
            "first_sha256": first.get(path),
            "second_sha256": second.get(path),
        }
        for path in sorted(first.keys() | second.keys())
        if first.get(path) != second.get(path)
    ]
    report = {
        "schema_version": 1,
        "first": {"path": str(args.first), "sha256": sha256_file(args.first)},
        "second": {"path": str(args.second), "sha256": sha256_file(args.second)},
        "byte_identical": sha256_file(args.first) == sha256_file(args.second),
        "member_differences": differences,
        "explanation": (
            "The deterministic packager normalizes ordering, ownership, modes, timestamps, and "
            "compression. Any remaining difference originates in a generated input or native binary "
            "and must be reviewed before publication."
        ),
    }
    write_json(args.output, report)
    print(json.dumps(report, sort_keys=True))
    if not report["byte_identical"] and args.require_identical:
        raise ArtifactError("independent rebuilds are not byte-identical")


def parser() -> argparse.ArgumentParser:
    """Construct the command-line parser."""
    result = argparse.ArgumentParser(description=__doc__)
    subcommands = result.add_subparsers(dest="command", required=True)

    assemble_parser = subcommands.add_parser("assemble", help="assemble a macOS artifact")
    assemble_parser.add_argument("--manifest", type=Path, required=True)
    assemble_parser.add_argument("--repository-root", type=Path, required=True)
    assemble_parser.add_argument("--webkit-root", type=Path, required=True)
    assemble_parser.add_argument("--target", required=True)
    assemble_parser.add_argument("--jsc-library", type=Path, required=True)
    assemble_parser.add_argument("--shim-library", type=Path, required=True)
    assemble_parser.add_argument("--staging", type=Path, required=True)
    assemble_parser.add_argument("--output", type=Path, required=True)
    assemble_parser.add_argument("--zstd", default="zstd")
    assemble_parser.set_defaults(function=assemble)

    verify_parser = subcommands.add_parser("verify", help="verify a macOS artifact")
    verify_parser.add_argument("--manifest", type=Path, required=True)
    verify_parser.add_argument("--target", required=True)
    verify_parser.add_argument("--archive", type=Path, required=True)
    verify_parser.add_argument("--sbom", type=Path, required=True)
    verify_parser.add_argument("--zstd", default="zstd")
    verify_parser.add_argument("--skip-macho", action="store_true", help=argparse.SUPPRESS)
    verify_parser.set_defaults(function=verify)

    compare_parser = subcommands.add_parser("compare", help="compare independent rebuilds")
    compare_parser.add_argument("--first", type=Path, required=True)
    compare_parser.add_argument("--second", type=Path, required=True)
    compare_parser.add_argument("--output", type=Path, required=True)
    compare_parser.add_argument("--zstd", default="zstd")
    compare_parser.add_argument("--require-identical", action="store_true")
    compare_parser.set_defaults(function=compare)
    return result


def main() -> int:
    """Run the selected artifact operation with concise diagnostics."""
    arguments = parser().parse_args()
    try:
        arguments.function(arguments)
    except ArtifactError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
