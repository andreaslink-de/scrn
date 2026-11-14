"""Writes latest.json, the update manifest of the Tauri updater, for a
client release. It reads the signed packages and their .sig files in the
given folder and points at their download URLs in the GitHub release.

Usage: latest_json.py <folder> <tag> <notes file>
"""

import datetime
import json
import pathlib
import sys

REPO = "https://github.com/andreaslink-de/scrn"

# Package suffix -> updater targets (os-arch-bundle, then os-arch).
TARGETS = {
    "_amd64.AppImage": ["linux-x86_64-appimage", "linux-x86_64"],
    "_amd64.deb": ["linux-x86_64-deb"],
    "_x64-setup.exe": ["windows-x86_64-nsis", "windows-x86_64"],
}


def main(folder: str, tag: str, notes: str) -> None:
    dist = pathlib.Path(folder)
    platforms = {}
    for suffix, targets in TARGETS.items():
        packages = sorted(dist.glob(f"*{suffix}"))
        if len(packages) != 1:
            sys.exit(f"expected one *{suffix}, found {len(packages)}")
        package = packages[0]
        signature = package.with_name(package.name + ".sig")
        if not signature.is_file():
            sys.exit(f"{signature.name} is missing")
        entry = {
            "signature": signature.read_text().strip(),
            "url": f"{REPO}/releases/download/{tag}/{package.name}",
        }
        for target in targets:
            platforms[target] = entry
    manifest = {
        "version": tag.removeprefix("client-v"),
        "notes": pathlib.Path(notes).read_text().strip(),
        "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "platforms": platforms,
    }
    (dist / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({k: v["url"] for k, v in platforms.items()}, indent=2))


if __name__ == "__main__":
    main(*sys.argv[1:4])
