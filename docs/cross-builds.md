# Builds for other architectures

Rackett is one self-contained executable. These builds put it on NAS boxes, Raspberry
Pis and other machines without a Rust toolchain.

| Archive | Rust target | For |
| --- | --- | --- |
| `linux-x86_64` | `x86_64-unknown-linux-musl` | 64-bit PCs, servers and most NAS boxes (Intel/AMD) |
| `linux-arm64` | `aarch64-unknown-linux-musl` | Raspberry Pi 3/4/5 on a 64-bit OS, ARM NAS boxes and servers |
| `linux-armv7` | `armv7-unknown-linux-musleabihf` | Raspberry Pi 2/3/4 on a 32-bit OS, older ARM NAS boxes |
| `linux-armv6` | `arm-unknown-linux-musleabihf` | Raspberry Pi Zero and Pi 1 |
| `linux-riscv64` | `riscv64gc-unknown-linux-musl` | 64-bit RISC-V boards |
| `windows-x86_64` | `x86_64-pc-windows-gnu` | Windows 10/11, 64-bit |
| `macos-arm64`, `macos-x86_64` | `aarch64-apple-darwin`, `x86_64-apple-darwin` | Apple silicon and Intel Macs (release workflow only) |

The Linux builds link musl statically, so one file runs on any distribution, including
NAS systems with an old C library. Rackett's allocator is mimalloc, so musl's slow
`malloc` does not apply. Each archive holds the binary, `LICENSE`, `NOTICE` and
`README.md`; Windows builds come as `.zip`, the rest as `.tar.gz`. Archives are about
5–7 MB.

## Building locally

The only requirement is Docker on a Linux x86-64 host:

```sh
tools/build_targets.sh                        # all Linux and Windows targets
tools/build_targets.sh aarch64-unknown-linux-musl   # just one
```

Archives and `SHA256SUMS` are written to `dist/`. The build cache lives in
`target/cross`; `CACHE=/some/dir` moves it and `JOBS=6` sets the compile jobs.

[`packaging/cross/Dockerfile`](../packaging/cross/Dockerfile) is the build image. It
holds Rust 1.99.1 with each target's standard library, plus Zig 0.15.2 (checksum
pinned) as the C compiler and linker through cargo-zigbuild 0.24.0. C code (`ring`,
mimalloc) therefore cross-compiles without a separate toolchain per target. The
image also carries qemu-user for testing.

Every Linux binary is tested before it is packaged. It prints `--version`, then
starts with an empty data directory on a random port and must answer `/health`
within 60 seconds, under qemu for the foreign architectures.

**Why some targets need special handling:**

- **macOS** cannot be cross-built here: mimalloc includes Apple SDK headers
  (`CommonCrypto`), which cannot be redistributed. The release workflow builds both Mac
  targets natively on a macOS runner.
- **Windows:** mimalloc's build script turns `-Wdate-time` into an error for Windows
  only, so the script passes `CFLAGS_x86_64_pc_windows_gnu=-Wno-error=date-time`.
  Its use of `__DATE__` only affects a version string.

## Beta releases on GitHub

[`.github/workflows/release.yml`](../.github/workflows/release.yml) (source copy in
`packaging/github/release.yml`) runs after "Rust checks" passes on `main`, or when
started by hand:

1. `cross`: `tools/build_targets.sh` builds the Linux and Windows archives, with the
   qemu health checks.
2. `macos`: native builds for Apple silicon and Intel. The Apple-silicon build answers
   `/health`. The Intel build is run under Rosetta when the runner has it.
3. `windows-smoke`: the cross-built Windows binary unpacks and must answer `/health`
   on a Windows runner.
4. `release`: if all of that passed, a prerelease `v<version>-beta.<run>` is published
   with every archive and a combined `SHA256SUMS`, pointing at the tested commit.

So every update of `main` that passes the checks gets its own beta release.

## Status and limits

Verified on 2026-11-03:

- **Linux:** all five Linux targets built, statically linked, and answered `/health`
  under qemu.
- **Windows:** the binary built as a PE32+ executable; it is tested only by the
  workflow's Windows runner.
- **macOS:** built only by the workflow.
- **Daily use:** only x86-64 Linux runs Rackett in daily use. The ARM, RISC-V, Windows
  and macOS builds pass the start-up checks above, but have no long-running or
  live-tracker testing yet.
- **Windows file permissions:** Rackett narrows the permissions of its data directory
  and of `ticket.key` to the owner on Unix only. On Windows those files keep the
  data directory's inherited permissions, so keep the data directory inside your user
  profile.
- **Windows durability:** saving settings or a session flushes the file and then renames
  it into place. On Unix the directory is flushed afterwards as well. Windows cannot
  open a directory to flush it, so there the file's own flush is the last guarantee
  (`store::sync_dir`). The first Windows beta run caught this: every save reported
  "durability could not be confirmed" and the first start refused to continue.
- **Speed:** a Raspberry Pi is slower per request than the desktop measured in
  [speed.md](speed.md). Rackett's CPU use per search is small, so this mostly shows up
  on large uncached result pages.
