#!/usr/bin/env bash
# CI on this machine: unit tests, a portable release build and a tarball to install with
# scripts/install.sh. Usage: scripts/ci.sh
#
# The speech engine (ggml) would otherwise be tuned to the build machine (-march=native), and a
# build with AVX-512 crashes with "illegal instruction" on most laptops. The floor here is
# x86-64-v3 (AVX2, FMA, F16C, BMI2): any Intel since 2013 (Haswell) or AMD since 2015.
set -euo pipefail
cd "$(dirname "$0")/.."

export TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=OFF -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON -DGGML_AVX512=OFF"

echo "== tests"
cargo test --quiet

echo "== release build"
cargo build --release
bin=target/release/hushnote

echo "== checks"
# The speech engine must stay within the floor. (Other crates, such as blake3, carry AVX-512
# paths they only take after checking the CPU at runtime; those are fine.)
engine=$(ls -td target/release/build/transcribe-cpp-sys-*/out/lib | head -1)
for lib in "$engine"/libggml*.a "$engine"/libtranscribe.a; do
    # (no `grep -q`: it closes the pipe early and pipefail then reports objdump's SIGPIPE)
    if [[ $(objdump -d --no-show-raw-insn "$lib" | grep -c '%zmm') -gt 0 ]]; then
        echo "$lib uses AVX-512; it would crash on most laptops" >&2
        exit 1
    fi
done
if [[ $(objdump -d --no-show-raw-insn "$engine/libggml-cpu.a" | grep -c '%ymm') -eq 0 ]]; then
    echo "the speech engine has no AVX2 code; it would be slow" >&2
    exit 1
fi
echo "speech engine: AVX2, no AVX-512"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
name="hushnote-$version-linux-x86_64"
rm -rf "dist/$name"
mkdir -p "dist/$name"
cp "$bin" "dist/$name/hushnote"
cp scripts/install.sh "dist/$name/install.sh"
cp assets/logo.png "dist/$name/hushnote.png"
tar -C dist -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
rm -rf "dist/$name"

echo "== done"
ls -la "dist/$name.tar.gz"
echo "binary: $(du -h "$bin" | cut -f1)  tarball: $(du -h "dist/$name.tar.gz" | cut -f1)"
