"""Validate the draft before uploading or publishing extension release assets."""
import json
from pathlib import Path
import sys
import tomllib


def metadata(manifest, tag):
    extension = manifest["extension"]
    if tag != f"v{extension['version']}":
        raise ValueError("release tag does not match extension.toml's version")
    platforms = extension["platforms"]
    if not isinstance(platforms, list) or not platforms or not all(isinstance(p, str) and p for p in platforms):
        raise ValueError("release needs a non-empty platform list")
    if len(set(platforms)) != len(platforms):
        raise ValueError("release platforms must be unique")
    return extension


def check(manifest, tag, release, *, assets=False):
    extension = metadata(manifest, tag)
    if release.get("isDraft") is not True or release.get("tagName") != tag:
        raise ValueError("the matching release must still be a draft")
    if assets:
        expected = sorted(f"{extension['id']}-{extension['version']}-{p}.tar.gz"
                          for p in extension["platforms"])
        uploaded = release.get("assets", [])
        actual = sorted(a["name"] for a in uploaded)
        if actual != expected or any(a.get("size", 0) <= 0 for a in uploaded):
            raise ValueError(f"release needs exactly these non-empty archives: {expected}; got {actual}")


def main():
    mode, manifest_path, tag = sys.argv[1:]
    manifest = tomllib.loads(Path(manifest_path).read_text())
    if mode == "metadata":
        extension = metadata(manifest, tag)
        print(f"id={extension['id']}\nversion={extension['version']}")
    elif mode in ("check-draft", "check-assets"):
        check(manifest, tag, json.load(sys.stdin), assets=mode == "check-assets")
    else:
        raise ValueError(f"unknown release validation mode: {mode}")


if __name__ == "__main__":
    main()
