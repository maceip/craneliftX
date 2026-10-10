#!/usr/bin/env bash
# Setup script for CraneliftX on Ubuntu 24.04 Linux
set -euo pipefail

echo "==> 1. Installing Linux dependencies (QEMU user emulator & RISC-V cross-compiler)..."
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    build-essential \
    clang-20 \
    llvm-20-dev \
    lld-20 \
    gcc-riscv64-linux-gnu \
    libc6-dev-riscv64-cross \
    qemu-user-static \
    nginx \
    python3 \
    python3-pip \
    python3-venv

echo "==> 2. Setting up Python virtual environment..."
python3 -m venv .venv
.venv/bin/pip install --upgrade pip
.venv/bin/pip install capstone protobuf

echo "==> 3. Verifying QEMU and RISC-V toolchain..."
riscv64-linux-gnu-gcc --version | head -n 1
qemu-riscv64-static --version | head -n 1

echo "==> 4. Building demo and executing RISC-V + Pulley lowering..."
make demo

echo "==> 5. Ready for systemd & Nginx configuration."
echo "Copy deploy/nginx.conf to /etc/nginx/sites-available/craneliftx"
echo "Copy deploy/craneliftx.service to /etc/systemd/system/craneliftx.service"
