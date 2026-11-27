#!/usr/bin/env bash
# usage: scripts/check-vendor.sh
#
# Every vendored crate (vendor/crates.txt) is its published tarball unchanged, apart from the
# changes marked `hypha patch` (vendor/README.md): the tarball's sha256 must be the pinned one,
# every file must be equal or differ only in hunks that add a line containing `hypha patch`,
# and no file may be missing or extra. The tarball comes from cargo's download cache, else from
# static.crates.io.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 -I - <<'EOF'
import difflib, hashlib, io, os, pathlib, sys, tarfile, urllib.request

# Files cargo adds or rewrites when it unpacks a crate; vendor/ does not keep them.
UNPACKED_ONLY = {".cargo-ok", ".cargo_vcs_info.json", "Cargo.toml.orig"}
MARK = "hypha patch"

def tarball(name, version):
    cache = pathlib.Path(os.environ.get("CARGO_HOME", pathlib.Path.home() / ".cargo"))
    for found in sorted((cache / "registry" / "cache").glob(f"*/{name}-{version}.crate")):
        return found.read_bytes()
    url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
    with urllib.request.urlopen(url, timeout=60) as response:
        return response.read()

failures = []
for line in pathlib.Path("vendor/crates.txt").read_text().splitlines():
    if not line.strip() or line.startswith("#"):
        continue
    directory, name, version, sha256 = line.split()
    data = tarball(name, version)
    if hashlib.sha256(data).hexdigest() != sha256:
        failures.append(f"{name} {version}: the tarball's sha256 is not the pinned one")
        continue
    upstream = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            if member.isfile():
                relative = member.name.split("/", 1)[1]
                if relative not in UNPACKED_ONLY:
                    upstream[relative] = archive.extractfile(member).read()
    root = pathlib.Path(directory)
    vendored = {
        str(path.relative_to(root)): path.read_bytes() for path in root.rglob("*") if path.is_file()
    }
    for missing in sorted(upstream.keys() - vendored.keys()):
        failures.append(f"{directory}/{missing}: missing")
    for extra in sorted(vendored.keys() - upstream.keys()):
        failures.append(f"{directory}/{extra}: not in {name} {version}")
    for relative in sorted(upstream.keys() & vendored.keys()):
        if upstream[relative] == vendored[relative]:
            continue
        old = upstream[relative].decode("utf-8", "replace").splitlines()
        new = vendored[relative].decode("utf-8", "replace").splitlines()
        matcher = difflib.SequenceMatcher(a=old, b=new, autojunk=False)
        for tag, _, _, b0, b1 in matcher.get_opcodes():
            if tag != "equal" and not any(MARK in added for added in new[b0:b1]):
                failures.append(f"{directory}/{relative}:{b0 + 1}: a change not marked '{MARK}'")

if failures:
    print("vendor/ differs from the published crates:", *failures, sep="\n  ", file=sys.stderr)
    sys.exit(1)
EOF
