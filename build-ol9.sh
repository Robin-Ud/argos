#!/bin/bash
# Build de release pra VM (Oracle Linux 9, glibc 2.34) dentro de um container oraclelinux:9.
# Compilar direto no Fedora gera binário que pede GLIBC_2.39 (pidfd_spawnp da libstd) e não sobe na VM.
# Saída: target/ol9/release/argos. Toolchain e registry ficam em volumes do podman (cache).
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# imagem local com gcc já instalado (o dnf baixando 54 MB a cada build travou no eduroam em 07/10);
# sem ela, cai na oraclelinux:9 limpa e instala o gcc na hora
IMG=localhost/argos-ol9-build
podman image exists "$IMG" || IMG=docker.io/library/oraclelinux:9
podman run --rm --security-opt label=disable \
    -v "$DIR":/src -w /src \
    -v argos-rustup:/root/.rustup -v argos-cargo:/root/.cargo \
    "$IMG" bash -c '
        set -e
        command -v gcc >/dev/null || dnf install -y -q gcc >/dev/null
        if [ ! -x /root/.cargo/bin/cargo ]; then
            curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal
        fi
        /root/.cargo/bin/cargo build --release --target-dir target/ol9
        ldd --version | head -1
    '
ls -la "$DIR/target/ol9/release/argos"
objdump -T "$DIR/target/ol9/release/argos" | grep -o "GLIBC_[0-9.]*" | sort -uV | tail -1
