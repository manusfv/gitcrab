"""Rebuild the checked-in crates from verified registry archives."""

import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
from tempfile import TemporaryDirectory
from urllib.request import urlretrieve

ROOT = Path(__file__).resolve().parents[1]
CRATES = {
    "gpui-component": (
        "0.5.1",
        "d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a",
        set(),
    ),
    "xattr": (
        "0.2.3",
        "6d1526bbe5aaeb5eb06885f4d987bcdfa5e23187055de9b83fe00156a821fabc",
        {".cargo_vcs_info.json", ".cirrus.yml", ".gitignore", ".travis.yml",
         "Cargo.toml.orig", "tests/main.rs"},
    ),
}


def contents(root, ignored):
    files = {}
    for path in root.rglob("*"):
        name = path.relative_to(root).as_posix()
        if path.is_file() and name not in ignored:
            data = path.read_bytes()
            # Git can translate text line endings at checkout.
            if b"\x00" not in data:
                try:
                    data.decode("utf-8")
                except UnicodeDecodeError:
                    pass
                else:
                    data = data.replace(b"\r\n", b"\n")
            files[name] = data
    return files


def verify(name, version, sha256, omitted, scratch, cache):
    stem = f"{name}-{version}"
    archive = (cache or scratch) / f"{stem}.crate"
    if cache is None:
        urlretrieve(f"https://static.crates.io/crates/{name}/{stem}.crate", archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != sha256:
        raise SystemExit(f"Checksum mismatch for {archive}")
    with tarfile.open(archive) as package:
        package.extractall(scratch, filter="data")
    vendor = ROOT / "vendor" / name
    upstream = scratch / stem
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    env["GIT_CEILING_DIRECTORIES"] = str(scratch)
    subprocess.run(["git", "apply", str(vendor / "gitcrab.patch")], cwd=upstream, env=env, check=True)
    expected = contents(upstream, omitted)
    actual = contents(vendor, {"GITCRAB-PATCH.md", "gitcrab.patch"})
    mismatch = [file for file in sorted(actual.keys() | expected.keys())
                if actual.get(file) != expected.get(file)]
    if mismatch:
        raise SystemExit(f"{name} differs from upstream plus patch: {', '.join(mismatch)}")
    print(f"Verified {stem} against its checksummed crate and patch")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--crate-cache", type=Path, help="Use local published .crate archives")
    args = parser.parse_args()
    with TemporaryDirectory(prefix="gitcrab-vendor-") as directory:
        for name, (version, sha256, omitted) in CRATES.items():
            verify(name, version, sha256, omitted, Path(directory), args.crate_cache)
