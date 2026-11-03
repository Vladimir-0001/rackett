#!/usr/bin/env bash
# Build release archives of Rackett for several architectures (docs/cross-builds.md).
#
#   tools/build_targets.sh [TARGET...]
#
# Runs in the packaging/cross image: Rust's prebuilt standard libraries, with Zig as
# C compiler and linker through cargo-zigbuild. Linux builds are static (musl), so one
# file runs on any distribution, NAS or Raspberry Pi OS. Each Linux binary is started
# under qemu-user and must answer /health. Archives and SHA256SUMS land in dist/.
# macOS cannot be cross-built here: mimalloc needs Apple SDK headers. The release
# workflow builds it on a macOS runner. Environment: OUT (default dist), CACHE (default
# target/cross; holds the target and Cargo directories), JOBS (default 4).
set -euo pipefail

ALL_TARGETS=(
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  armv7-unknown-linux-musleabihf
  arm-unknown-linux-musleabihf
  riscv64gc-unknown-linux-musl
  x86_64-pc-windows-gnu
)

# Archive label per target.
label() {
  case "$1" in
    x86_64-unknown-linux-musl) echo linux-x86_64 ;;
    aarch64-unknown-linux-musl) echo linux-arm64 ;;
    armv7-unknown-linux-musleabihf) echo linux-armv7 ;;
    arm-unknown-linux-musleabihf) echo linux-armv6 ;;
    riscv64gc-unknown-linux-musl) echo linux-riscv64 ;;
    x86_64-pc-windows-gnu) echo windows-x86_64 ;;
    *) echo "unknown target $1" >&2; return 1 ;;
  esac
}
# qemu-user program that runs a target's binary (empty: native).
emulator() {
  case "$1" in
    aarch64-*) echo qemu-aarch64 ;;
    armv7-* | arm-*) echo qemu-arm ;;
    riscv64*) echo qemu-riscv64 ;;
    *) echo "" ;;
  esac
}

inside() {
  local version targets=("$@")
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  mkdir -p /out
  for target in "${targets[@]}"; do
    local name bin dir
    name="rackett-${version}-$(label "$target")"
    echo "== $target -> $name"
    # mimalloc's build adds -Werror=date-time for Windows only; its __DATE__ use is harmless.
    CFLAGS_x86_64_pc_windows_gnu=-Wno-error=date-time \
      cargo zigbuild --release --locked --bin rackett --target "$target"
    bin=/target/$target/release/rackett
    [[ $target == *windows* ]] && bin=$bin.exe
    file "$bin"
    if [[ $target == *linux* ]]; then
      smoke "$target" "$bin"
    fi
    dir=$(mktemp -d)/$name
    mkdir -p "$dir"
    cp "$bin" LICENSE NOTICE README.md "$dir/"
    if [[ $target == *windows* ]]; then
      (cd "$(dirname "$dir")" && rm -f "/out/$name.zip" && zip -qr "/out/$name.zip" "$name")
    else
      tar -C "$(dirname "$dir")" -czf "/out/$name.tar.gz" "$name"
    fi
  done
  (cd /out && sha256sum rackett-* | grep -v SHA256SUMS > SHA256SUMS && cat SHA256SUMS)
}

# Start the binary (under qemu for foreign architectures) with an empty data directory
# and require /health within 60 seconds; the version must print too.
smoke() {
  local target=$1 bin=$2 run data port pid
  run=$(emulator "$target")
  $run "$bin" --version
  data=$(mktemp -d)
  port=$((20000 + RANDOM % 20000))
  $run "$bin" --data-dir "$data" --port "$port" >"$data/log" 2>&1 &
  pid=$!
  for _ in $(seq 1 120); do
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
      kill "$pid"; wait "$pid" 2>/dev/null || true
      echo "smoke: $target answered /health"
      return 0
    fi
    sleep 0.5
  done
  kill "$pid" 2>/dev/null || true
  cat "$data/log" >&2
  echo "smoke: $target did not answer /health" >&2
  return 1
}

if [[ ${1:-} == --inside ]]; then
  shift
  inside "$@"
  exit
fi

cd "$(dirname "$0")/.."
OUT=${OUT:-dist}
CACHE=${CACHE:-target/cross}
mkdir -p "$OUT" "$CACHE/target" "$CACHE/cargo"
CACHE=$(cd "$CACHE" && pwd)
targets=("$@")
[[ ${#targets[@]} -gt 0 ]] || targets=("${ALL_TARGETS[@]}")
for t in "${targets[@]}"; do label "$t" >/dev/null; done
docker build -q -t rackett-cross packaging/cross >/dev/null
docker run --rm -u "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="${JOBS:-4}" \
  -v "$PWD:/src:ro" -v "$CACHE/target:/target" -v "$CACHE/cargo:/cargo" \
  -v "$(cd "$OUT" && pwd):/out" -w /src \
  rackett-cross bash -c 'export PATH=/usr/local/cargo/bin:$PATH; exec tools/build_targets.sh --inside "$@"' _ "${targets[@]}"
