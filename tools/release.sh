#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
node --check web/app.js
cargo build --locked --release
release_version="$(target/release/rackett --version | awk '{print $2}')"
release_target="$(rustc -vV | sed -n 's/^host: //p')"
release_name="rackett-${release_version}-${release_target}"
release_stage="$(mktemp -d)"
trap 'rm -rf "$release_stage"' EXIT
mkdir -p "dist" "$release_stage/$release_name"
cp target/release/rackett README.md LICENSE NOTICE "$release_stage/$release_name/"
cp -R docs packaging vendor "$release_stage/$release_name/"
mkdir -p "$release_stage/$release_name/source"
cp Cargo.toml Cargo.lock build.rs LICENSE NOTICE README.md "$release_stage/$release_name/source/"
cp -R src web assets tests tools docs packaging vendor examples .github "$release_stage/$release_name/source/"
find "$release_stage/$release_name/source" -type d -name __pycache__ -prune -exec rm -rf {} +
tar -C "$release_stage" -czf "dist/$release_name.tar.gz" "$release_name"
(cd dist && sha256sum "$release_name.tar.gz" > "$release_name.tar.gz.sha256")
printf 'Built %s\n' "dist/$release_name.tar.gz"
