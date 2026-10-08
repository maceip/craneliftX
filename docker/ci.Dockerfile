# Toolchain image for the main-branch release job.
# Cross compilers, Rust, Syft, and LLVM 18 are installed here so a run does
# not apt-get. clang-18 is installed next to llvm-link because remill's
# bitcode compiler search does not look on PATH. Source is mounted at /src;
# this file does not COPY the repo, so the layer cache stays valid across
# code changes.

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin \
    CARGO_TERM_COLOR=always \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
    CARGO_INCREMENTAL=0 \
    PKG_CONFIG_ALLOW_CROSS=1 \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CC_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-gcc \
    AR_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-ar

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        binutils \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        file \
        gcc-aarch64-linux-gnu \
        gcc-riscv64-linux-gnu \
        git \
        libc6-dev \
        libc6-dev-arm64-cross \
        libc6-dev-riscv64-cross \
        clang-18 \
        llvm-18-dev \
        ninja-build \
        pkg-config \
        python3 \
        zlib1g-dev \
    && test -x /usr/lib/llvm-18/bin/llvm-link \
    && test -x /usr/lib/llvm-18/bin/clang++ \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain 1.99.0 --profile minimal \
    && rustup target add aarch64-unknown-linux-gnu riscv64gc-unknown-linux-gnu \
    && chmod -R a+w "${RUSTUP_HOME}" "${CARGO_HOME}"

ARG SYFT_VERSION=1.54.1
ARG SYFT_SHA256=c069905b391cc4c20a5ba65ad5c10be2a7ba074f8ea6ad203e24d14e303dad47
RUN curl -fsSL -o /tmp/syft.tar.gz \
        "https://github.com/anchore/syft/releases/download/v${SYFT_VERSION}/syft_${SYFT_VERSION}_linux_amd64.tar.gz" \
    && echo "${SYFT_SHA256}  /tmp/syft.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/syft.tar.gz -C /usr/local/bin syft \
    && rm /tmp/syft.tar.gz \
    && syft version

COPY cargo-config.toml /usr/local/cargo/config.toml

WORKDIR /src
