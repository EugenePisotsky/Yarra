#!/usr/bin/env python3
"""Builds the Blender profile the character scripts run in: MPFB and its asset packs.

Usage: python3 content/characters/scripts/setup_blender.py [--replace]

Downloads the packs content/characters/mpfb.toml pins into .tools/downloads/<id>/ (kept, and
checked against their hashes, between runs), then makes a new profile at .tools/blender: MPFB from
extensions.blender.org through Blender's extension client, which checks the archive against the
index, refused unless the index lists the pinned version and hash; the packs unpacked into MPFB's
data folder. The profile leaves the user's own Blender untouched; scripts run in it with
BLENDER_USER_RESOURCES=.tools/blender. --replace deletes an existing profile first.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "content/characters/mpfb.toml"
DOWNLOADS = ROOT / ".tools/downloads"
PROFILE = ROOT / ".tools/blender"
REPOSITORY = PROFILE / "extensions/blender_org"
BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"


def sha256(path):
    digest = hashlib.sha256()

    with open(path, "rb") as file:
        while chunk := file.read(1 << 20):
            digest.update(chunk)

    return digest.hexdigest()


def fetch(pack):
    """The pinned pack, downloaded unless an intact copy is already there."""
    path = DOWNLOADS / pack["id"] / pack["url"].rsplit("/", 1)[1]

    if path.exists() and sha256(path) == pack["sha256"]:
        return path

    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".part")
    print(f"downloading {pack['id']}")
    subprocess.run(["curl", "-sSL", "--fail", "-o", partial, pack["url"]], check=True)
    actual = sha256(partial)

    if actual != pack["sha256"]:
        partial.unlink()
        raise SystemExit(f"{pack['id']}: sha256 {actual}, expected {pack['sha256']}")

    partial.replace(path)
    return path


def blender(*args, capture=False):
    env = dict(os.environ, BLENDER_USER_RESOURCES=str(PROFILE))
    result = subprocess.run([BLENDER, *args], env=env, check=True, capture_output=capture, text=True)
    return result.stdout


def install_mpfb(pinned):
    """Installs MPFB from the official index once its listing matches the pin."""
    if "blender_org:" not in blender("--command", "extension", "repo-list", capture=True):
        url = "https://extensions.blender.org/api/v1/extensions/"
        blender("--command", "extension", "repo-add", "--url", url, "blender_org")

    blender("--online-mode", "--command", "extension", "sync")
    index = json.loads((REPOSITORY / ".blender_ext/index.json").read_text())
    listed = next(entry for entry in index["data"] if entry["id"] == "mpfb")
    expected = (pinned["version"], f"sha256:{pinned['sha256']}")

    if (listed["version"], listed["archive_hash"]) != expected:
        raise SystemExit(
            f"extensions.blender.org lists MPFB {listed['version']} ({listed['archive_hash']}), "
            f"pinned {pinned['version']}; check the new version, then update mpfb.toml"
        )

    blender("--online-mode", "--command", "extension", "install", "-e", "blender_org.mpfb")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--replace", action="store_true", help="delete an existing profile first")
    replace = parser.parse_args().replace

    if PROFILE.exists() and not replace:
        raise SystemExit(f"{PROFILE} exists; pass --replace to rebuild it")

    manifest = tomllib.loads(MANIFEST.read_text())
    packs = [fetch(pack) for pack in manifest["pack"]]

    shutil.rmtree(PROFILE, ignore_errors=True)
    PROFILE.mkdir(parents=True)
    install_mpfb(manifest["mpfb"])

    install = ROOT / "content/characters/scripts/install_packs.py"
    blender("-b", "--python-exit-code", "1", "--python", str(install), "--", *map(str, packs))
    print(f"SETUP_DONE MPFB {manifest['mpfb']['version']}, {len(packs)} packs")


if __name__ == "__main__":
    main()
