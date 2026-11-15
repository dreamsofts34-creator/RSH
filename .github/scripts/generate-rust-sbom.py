#!/usr/bin/env python3
"""Generate the RSH Rust SBOM (`sbom/rust.cdx.json`) from the full Cargo.lock resolve.

Why this script exists instead of `cargo cyclonedx` (deep-scan Round 2, M1 —
proven locally 2026-11-14 with cargo-cyclonedx 0.5.10):
  * `cargo cyclonedx --format json` writes ONE `<crate>.cdx.json` PER workspace
    member next to that member's Cargo.toml (14 files here). It never writes a
    root `bom.json`, so the old release step (`Copy-Item bom.json ...`) failed
    fail-closed on a missing file.
  * Its default spec version is 1.3, not 1.6 (needs `--spec-version 1.6`).
  * Even merged, its union covers only the shipped dependency graph: the
    host-target run unions to 420 (name, version) pairs and `--target all`
    to 589 — dev-dependencies (criterion, clap for benches, ...) are
    excluded by design. The committed SBOM is the FULL lockfile resolve
    (626 packages), which only `cargo metadata` reproduces.
  * So: this script reads `cargo metadata` (full resolve, `--locked`) and
    emits one CycloneDX 1.6 file in the exact shape H4 established
    (uniform `{type, bom-ref, name, version, purl, licenses}` components,
    `pkg:cargo/<name>@<version>` bom-refs, single SPDX id as
    `{"license": {"id": ...}}`, anything else as `{"expression": ...}`).

Fail-closed: the component count must equal the `[[package]]` count in
Cargo.lock, and every package must declare a license; either mismatch
exits non-zero. A package without a license is an error, never a
licenseless component.

Usage (repo root):
  python .github/scripts/generate-rust-sbom.py sbom/rust.cdx.json
  python .github/scripts/generate-rust-sbom.py release-staging/sbom-rust.cdx.json
Requires: cargo on PATH, Python 3.11+ (tomllib). No network: `--locked`
and `--offline` are not needed — `cargo metadata` reads the lockfile.
"""

import json
import re
import subprocess
import sys
import tomllib
import uuid
from datetime import datetime, timezone
from pathlib import Path


def fail(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: generate-rust-sbom.py <output-path>")
    out_path = Path(sys.argv[1])
    root = Path.cwd()

    cargo_toml = root / "Cargo.toml"
    if not cargo_toml.is_file():
        fail("Cargo.toml not found — run from the repo root")
    with open(cargo_toml, "rb") as f:
        workspace_version = tomllib.load(f)["workspace"]["package"]["version"]

    lock_path = root / "Cargo.lock"
    lock_text = lock_path.read_text(encoding="utf-8")
    lock_count = len(re.findall(r"^\[\[package\]\]", lock_text, re.MULTILINE))

    # Dry run #10 (2026-11-15): `text=True` decoded cargo metadata with
    # the Windows locale codec (cp1252) and crashed on byte 0x81
    # (UnicodeDecodeError, then json.loads(None) TypeError) — and the
    # PowerShell step swallowed it and printed "SBOMs regenerated"
    # anyway, so no sbom-rust.cdx.json was ever written. Capture BYTES
    # and decode UTF-8 explicitly (cargo metadata is JSON, UTF-8 by
    # spec); a decode failure is fail-closed here, never
    # errors="replace" on the data. stderr is only ever shown inside
    # an error message, so replace there is fine.
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=root,
            capture_output=True,
        )
    except FileNotFoundError:
        fail("cargo not found on PATH")
    if result.returncode != 0:
        fail(f"cargo metadata failed:\n{result.stderr.decode('utf-8', errors='replace')}")
    try:
        stdout_text = result.stdout.decode("utf-8")
    except UnicodeDecodeError as e:
        fail(f"cargo metadata output is not valid UTF-8: {e}")
    metadata = json.loads(stdout_text)
    packages = metadata["packages"]

    if len(packages) != lock_count:
        fail(
            f"cargo metadata lists {len(packages)} packages but Cargo.lock "
            f"has {lock_count} [[package]] entries — lockfile out of sync, "
            "run `cargo update` / `cargo metadata` and re-check"
        )

    components = []
    seen: set[tuple[str, str]] = set()
    for pkg in sorted(packages, key=lambda p: (p["name"].lower(), p["version"])):
        key = (pkg["name"], pkg["version"])
        if key in seen:
            fail(f"duplicate package in resolve: {key[0]} {key[1]}")
        seen.add(key)
        license_str = pkg.get("license")
        if not license_str:
            fail(
                f"package {pkg['name']} {pkg['version']} declares no license "
                "(license or license_file required — see deny.toml)"
            )
        # A bare SPDX id stays an id; anything compound (OR/AND, slashes,
        # parens) is an expression — same rule H4's generator used.
        if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.\-+]*", license_str):
            licenses = [{"license": {"id": license_str}}]
        else:
            licenses = [{"expression": license_str}]
        components.append(
            {
                "type": "library",
                "bom-ref": f"pkg:cargo/{pkg['name']}@{pkg['version']}",
                "name": pkg["name"],
                "version": pkg["version"],
                "purl": f"pkg:cargo/{pkg['name']}@{pkg['version']}",
                "licenses": licenses,
            }
        )

    # Top-level key order matches the committed H4 file exactly
    # ($schema, bomFormat, specVersion, serialNumber, version, metadata,
    # components) so regenerations diff minimally.
    bom = {
        "$schema": "http://cyclonedx.org/schema/bom-1.6.schema.json",
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "tools": [
                {
                    "vendor": "RSH",
                    "name": "cargo-metadata SBOM generator",
                    "version": "1.1",
                }
            ],
            "component": {
                "type": "application",
                "bom-ref": f"pkg:cargo/rsh-app@{workspace_version}",
                "name": "rsh-app",
                "version": workspace_version,
                "description": (
                    f"RSH (Real Sim Hub) v{workspace_version} — Tauri 2 app; "
                    "Rust workspace SBOM generated from Cargo.lock via cargo "
                    f"metadata (full lockfile resolve, {len(components)} packages). "
                    "The Windows release tree (cargo tree -p rsh-app --target "
                    "x86_64-pc-windows-msvc) is a 299-package subset; see "
                    "THIRD-PARTY-LICENSES.md. Regenerate at release time with "
                    ".github/scripts/generate-rust-sbom.py (cargo cyclonedx "
                    "does NOT reproduce this file — per-crate BOMs, host-target "
                    "only, dev-deps excluded; see docs/DECISIONS.md §16)."
                ),
            },
            "licenses": [{"license": {"id": "MIT"}}],
        },
        "components": components,
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    # indent=2 matches the committed H4 file, so regenerations diff
    # minimally (serial/timestamp/description + real component deltas).
    out_path.write_text(json.dumps(bom, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out_path}: {len(components)} components (Cargo.lock: {lock_count})")


if __name__ == "__main__":
    main()
