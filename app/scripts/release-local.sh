#!/usr/bin/env bash
# Builds every release file on a Mac, for when GitHub Actions can't run:
#   macOS arm64 + x64 (.dmg), Windows x64 (NSIS setup.exe, cross-compiled with cargo-xwin),
#   CLI archives for the same three targets.
# Output: release/<version>/ at the repository root.
#
# Needs: Homebrew `rustup` (keg-only), `llvm`, `lld`, `nsis`; `cargo install cargo-xwin`;
# rustup targets x86_64-apple-darwin and x86_64-pc-windows-msvc.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$HOME/.cargo/bin:$PATH"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
out="$root/release/$version"
mkdir -p "$out"
cd "$root/app"
pnpm install --frozen-lockfile

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  pnpm sidecars "$target"
  pnpm tauri build --target "$target" --bundles app,dmg
  cp "$root/target/$target/release/bundle/dmg/"*.dmg "$out/"
done

target=x86_64-pc-windows-msvc
pnpm sidecars "$target"
pnpm tauri build --runner cargo-xwin --target "$target" --bundles nsis
cp "$root/target/$target/release/bundle/nsis/"*-setup.exe "$out/"

cd "$root"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p url2mp3 --target "$target"
  tar czf "$out/url2mp3-cli-$target.tar.gz" -C "target/$target/release" url2mp3
done
cargo xwin build --release --locked -p url2mp3 --target x86_64-pc-windows-msvc
(cd target/x86_64-pc-windows-msvc/release && zip -q "$out/url2mp3-cli-x86_64-pc-windows-msvc.zip" url2mp3.exe)

(cd "$out" && shasum -a 256 -- * > SHA256SUMS)
ls -lh "$out"
