#!/usr/bin/env bash
# Builds every release file on a Mac, for when GitHub Actions can't run:
#   macOS arm64 + x64 (.dmg), Windows x64 (NSIS setup.exe, cross-compiled with cargo-xwin),
#   Android arm64 + armv7 (.apk, signed), CLI archives for macOS and Windows.
# Output: release/<version>/ at the repository root.
#
# Needs: Homebrew `rustup` (keg-only), `llvm`, `lld`, `nsis`, `openjdk@21`; `cargo install
# cargo-xwin`; rustup targets x86_64-apple-darwin, x86_64-pc-windows-msvc, aarch64-linux-android,
# armv7-linux-androideabi; Android SDK + NDK; the signing key (~/.url2mp3/keystore.properties).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$HOME/.cargo/bin:$PATH"
export JAVA_HOME=${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Library/Android/sdk}
export NDK_HOME=${NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
out="$root/release/$version"
mkdir -p "$out"
cd "$root/app"
pnpm install --frozen-lockfile

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  pnpm sidecars "$target"
  pnpm tauri build --target "$target" --bundles app,dmg
  cp "$root/target/$target/release/bundle/dmg/"*"_${version}_"*.dmg "$out/"
done

target=x86_64-pc-windows-msvc
pnpm sidecars "$target"
pnpm tauri build --runner cargo-xwin --target "$target" --bundles nsis
cp "$root/target/$target/release/bundle/nsis/"*"_${version}_"*-setup.exe "$out/"

pnpm tauri android build --apk --split-per-abi --target aarch64 --target armv7
apks="$root/app/src-tauri/gen/android/app/build/outputs/apk"
cp "$apks/arm64/release/app-arm64-release.apk" "$out/url2mp3_${version}_android-arm64.apk"
cp "$apks/arm/release/app-arm-release.apk" "$out/url2mp3_${version}_android-armv7.apk"

cd "$root"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p url2mp3 --target "$target"
  tar czf "$out/url2mp3-cli-$target.tar.gz" -C "target/$target/release" url2mp3
done
cargo xwin build --release --locked -p url2mp3 --target x86_64-pc-windows-msvc
(cd target/x86_64-pc-windows-msvc/release && zip -q "$out/url2mp3-cli-x86_64-pc-windows-msvc.zip" url2mp3.exe)

(cd "$out" && shasum -a 256 -- * > SHA256SUMS)
ls -lh "$out"
