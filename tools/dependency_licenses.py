#!/usr/bin/env python3
"""Print license metadata for the complete locked Cargo dependency graph."""
import json
import subprocess

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
))
print(json.dumps([
    {"name": package["name"], "version": package["version"],
     "license": package["license"], "license_file": package["license_file"],
     "repository": package["repository"]}
    for package in sorted(metadata["packages"], key=lambda package: (package["name"], package["version"]))
], indent=2))
