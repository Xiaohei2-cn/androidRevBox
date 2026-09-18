#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 aarch64" >&2
}

fail() {
  echo "error: $*" >&2
  exit 1
}

architecture="${1:-}"
if [[ $# -ne 1 || "$architecture" != "aarch64" ]]; then
  usage
  exit 2
fi

command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v rustup >/dev/null 2>&1 || fail "rustup is required"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_root="$(cd "$script_dir/.." && pwd)"
target_triple="aarch64-linux-android"
android_api="${ANDROID_API:-21}"

if [[ ! "$android_api" =~ ^[0-9]+$ ]] || (( android_api < 21 )); then
  fail "ANDROID_API must be an integer >= 21 for Android arm64"
fi

if ! rustup target list --installed | grep -Fxq "$target_triple"; then
  fail "Rust target $target_triple is not installed; run: rustup target add $target_triple"
fi

ndk_candidates=()
if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  ndk_candidates+=("$ANDROID_NDK_HOME")
fi
if [[ -n "${ANDROID_NDK_ROOT:-}" ]]; then
  ndk_candidates+=("$ANDROID_NDK_ROOT")
fi

sdk_roots=()
if [[ -n "${ANDROID_SDK_ROOT:-}" ]]; then
  sdk_roots+=("$ANDROID_SDK_ROOT")
fi
if [[ -n "${ANDROID_HOME:-}" ]]; then
  sdk_roots+=("$ANDROID_HOME")
fi
sdk_roots+=("$HOME/Library/Android/sdk" "$HOME/Android/Sdk")

for sdk_root in "${sdk_roots[@]}"; do
  if [[ -d "$sdk_root/ndk" ]]; then
    while IFS= read -r ndk_dir; do
      ndk_candidates+=("$ndk_dir")
    done < <(find "$sdk_root/ndk" -mindepth 1 -maxdepth 1 -type d -print | sort -r)
  fi
done

ndk_root=""
toolchain_bin=""
linker_name="${target_triple}${android_api}-clang"
for candidate in "${ndk_candidates[@]}"; do
  [[ -f "$candidate/source.properties" ]] || continue
  while IFS= read -r prebuilt_bin; do
    if [[ -x "$prebuilt_bin/$linker_name" ]]; then
      ndk_root="$candidate"
      toolchain_bin="$prebuilt_bin"
      break 2
    fi
  done < <(find "$candidate/toolchains/llvm/prebuilt" -mindepth 2 -maxdepth 2 -type d -name bin -print 2>/dev/null)
done

if [[ -z "$ndk_root" ]]; then
  fail "no installed Android NDK contains $linker_name; set ANDROID_NDK_HOME to an existing NDK"
fi

linker="$toolchain_bin/$linker_name"
archiver="$toolchain_bin/llvm-ar"
readelf="$toolchain_bin/llvm-readelf"
[[ -x "$archiver" ]] || fail "NDK archiver is missing: $archiver"
[[ -x "$readelf" ]] || fail "NDK readelf is missing: $readelf"

cd "$workspace_root"
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$linker" \
CC_aarch64_linux_android="$linker" \
AR_aarch64_linux_android="$archiver" \
  cargo build -p android-agent --bin android-agent --release --target "$target_triple"

cargo_target_dir="${CARGO_TARGET_DIR:-$workspace_root/target}"
if [[ "$cargo_target_dir" != /* ]]; then
  cargo_target_dir="$workspace_root/$cargo_target_dir"
fi
agent_path="$cargo_target_dir/$target_triple/release/android-agent"
[[ -f "$agent_path" ]] || fail "Cargo reported success but Agent binary is missing: $agent_path"

elf_header="$("$readelf" -h "$agent_path")"
if ! grep -Eq 'Type:[[:space:]]+DYN' <<<"$elf_header"; then
  fail "Agent binary is not an Android PIE (ELF type DYN): $agent_path"
fi

if command -v shasum >/dev/null 2>&1; then
  sha256="$(shasum -a 256 "$agent_path" | awk '{print $1}')"
elif command -v sha256sum >/dev/null 2>&1; then
  sha256="$(sha256sum "$agent_path" | awk '{print $1}')"
else
  fail "shasum or sha256sum is required to report the artifact checksum"
fi

resource_dir="$workspace_root/src-tauri/resources/android-agent/aarch64"
resource_path="$resource_dir/android-agent"
resource_temp="$resource_dir/.android-agent.tmp.$$"
mkdir -p "$resource_dir"
trap 'rm -f "$resource_temp"' EXIT
cp "$agent_path" "$resource_temp"
chmod 755 "$resource_temp"
mv -f "$resource_temp" "$resource_path"
trap - EXIT

echo "target=$target_triple"
echo "android_api=$android_api"
echo "ndk=$ndk_root"
echo "agent_path=$agent_path"
echo "resource_path=$resource_path"
echo "sha256=$sha256"
