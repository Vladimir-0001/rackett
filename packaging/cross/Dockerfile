# Cross-compilation image for tools/build_targets.sh (docs/cross-builds.md).
# Rust's prebuilt standard libraries plus Zig as the C compiler and linker, through
# cargo-zigbuild, build every target from one Linux x86-64 host. qemu-user runs the
# foreign Linux binaries for the smoke test.
FROM rust:1.99.1-slim-bookworm

ARG ZIG_VERSION=0.15.2
ARG ZIG_SHA256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
ARG ZIGBUILD_VERSION=0.24.0

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl xz-utils file zip qemu-user \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL -o /tmp/zig.tar.xz \
      "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" \
 && echo "${ZIG_SHA256}  /tmp/zig.tar.xz" | sha256sum -c - \
 && mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 \
 && rm /tmp/zig.tar.xz
ENV PATH="/opt/zig:${PATH}"

RUN cargo install --locked cargo-zigbuild --version "${ZIGBUILD_VERSION}" \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

RUN rustup target add \
      x86_64-unknown-linux-musl \
      aarch64-unknown-linux-musl \
      armv7-unknown-linux-musleabihf \
      arm-unknown-linux-musleabihf \
      riscv64gc-unknown-linux-musl \
      x86_64-pc-windows-gnu \
      aarch64-apple-darwin \
      x86_64-apple-darwin
