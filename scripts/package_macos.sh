#!/usr/bin/env bash
# 本机构建 macOS 安装包（.app + .dmg），可选直接装进 /Applications。
# 与 .github/workflows/release.yml 共用同一套事实：先把设备端 Agent 打进
# resources/，再交给 tauri build；漏了这一步，干净克隆的包里就没有 Agent。
set -euo pipefail

usage() {
  echo "usage: $0 [--install] [--skip-agent] [--target <triple>]" >&2
  echo "  --install     构建后拷进 /Applications（无权限则 ~/Applications）并启动" >&2
  echo "  --skip-agent  不重建 Agent，沿用 resources/ 里已有的二进制" >&2
  echo "  --target      Rust target triple，默认按当前机器推断" >&2
}

install_app=false
skip_agent=false
target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install) install_app=true; shift ;;
    --skip-agent) skip_agent=true; shift ;;
    --target)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      target="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || { echo "error: 仅支持 macOS" >&2; exit 1; }

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_root="$(cd "$script_dir/.." && pwd)"
cd "$workspace_root"

command -v pnpm >/dev/null 2>&1 || { echo "error: 需要 pnpm（package.json 里就是 pnpm 脚本）" >&2; exit 1; }
command -v node >/dev/null 2>&1 || { echo "error: 需要 node" >&2; exit 1; }

if [[ -z "$target" ]]; then
  case "$(uname -m)" in
    arm64) target="aarch64-apple-darwin" ;;
    x86_64) target="x86_64-apple-darwin" ;;
    *) echo "error: 不认识的机器架构 $(uname -m)；用 --target 指定" >&2; exit 1 ;;
  esac
fi

version="$(node -p "require('./package.json').version")"
app_name="AppReverseTools"
agent_resource="src-tauri/resources/android-agent/aarch64/android-agent"

# 发布产物里不能出现构建机的家目录：把 cargo / rustup / 工作区路径统一重映射。
# （env!("CARGO_MANIFEST_DIR") 这种值不受 remap 影响，已在源码里按 debug 隔离。）
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}/.cargo=/cargo --remap-path-prefix=${HOME}/.rustup=/rustup --remap-path-prefix=${workspace_root}=/build"

# Agent 资源：有 NDK 就重建，没有就沿用现有二进制（但绝不能空着进包）。
if [[ "$skip_agent" == false ]]; then
  has_ndk=false
  for candidate in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_ROOT:-}" \
    "${ANDROID_HOME:-}/ndk" "${ANDROID_SDK_ROOT:-}/ndk" \
    "$HOME/Library/Android/sdk/ndk" "$HOME/Android/Sdk/ndk"; do
    if [[ -n "$candidate" && -d "$candidate" ]]; then
      has_ndk=true
      break
    fi
  done
  if [[ "$has_ndk" == true ]]; then
    echo "==> 重建设备端 Agent"
    bash scripts/build_android_agent.sh aarch64
  elif [[ -f "$agent_resource" ]]; then
    echo "==> 未检测到 Android NDK，沿用现有 Agent：$agent_resource" >&2
  else
    echo "error: 没有 NDK，也没有 ${agent_resource}；先运行 scripts/build_android_agent.sh aarch64" >&2
    exit 1
  fi
fi
[[ -f "$agent_resource" ]] || { echo "error: Agent 资源缺失，拒绝产出空包" >&2; exit 1; }

echo "==> tauri build（target=$target, version=${version}）"
pnpm tauri build --target "$target"

# 这是 Cargo workspace：产物落在根 target/；单独构建 src-tauri 时才在 src-tauri/target/。
bundle_root=""
for candidate in "target/$target/release/bundle" "src-tauri/target/$target/release/bundle"; do
  if [[ -d "$candidate/macos" ]]; then
    bundle_root="$candidate"
    break
  fi
done
[[ -n "$bundle_root" ]] || { echo "error: 构建成功但没找到 bundle 目录（target/$target/release/bundle）" >&2; exit 1; }

app_path="$bundle_root/macos/$app_name.app"
dmg_path="$(find "$bundle_root/dmg" -maxdepth 1 -name "$app_name""_$version""_*.dmg" -print -quit 2>/dev/null || true)"

[[ -d "$app_path" ]] || { echo "error: 没有找到 $app_path" >&2; exit 1; }
if [[ -f "$agent_resource" && ! -f "$app_path/Contents/Resources/aarch64/android-agent" ]]; then
  echo "error: 包内缺少 Contents/Resources/aarch64/android-agent（resources 映射没生效）" >&2
  exit 1
fi

# 发布产物不能带构建机的家目录。remap 失效时（比如有人绕过本脚本直接 tauri build）
# 这里会当场拦下，而不是等安装包上传到 Release 才发现。
binary_path="$(find "$app_path/Contents/MacOS" -maxdepth 1 -type f -print -quit)"
for shipped in "$binary_path" "$app_path/Contents/Resources/aarch64/android-agent"; do
  [[ -f "$shipped" ]] || continue
  if strings "$shipped" | grep -q -- "$HOME/"; then
    echo "error: $(basename "$shipped") 里仍带构建机路径 $HOME/（检查 RUSTFLAGS remap）" >&2
    exit 1
  fi
done

echo
echo "app:  $workspace_root/$app_path"
[[ -n "$dmg_path" ]] && echo "dmg:  $workspace_root/$dmg_path"

if [[ "$install_app" == false ]]; then
  echo
  echo "安装：把 $app_name.app 拖进「应用程序」，或重跑本脚本加 --install"
  exit 0
fi

# 安装：先退掉正在跑的实例，再整包替换，避免 WebView 进程占着旧二进制。
echo "==> 安装到 /Applications"
if pgrep -f "/$app_name.app/Contents/MacOS/" >/dev/null 2>&1; then
  osascript -e "tell application \"$app_name\" to quit" >/dev/null 2>&1 || true
  sleep 1
  pkill -f "/$app_name.app/Contents/MacOS/" >/dev/null 2>&1 || true
fi

if [[ -w /Applications ]]; then
  install_dir="/Applications"
else
  install_dir="$HOME/Applications"
  mkdir -p "$install_dir"
fi

installed_app="$install_dir/$app_name.app"
rm -rf "$installed_app"
ditto "$app_path" "$installed_app"
xattr -dr com.apple.quarantine "$installed_app" >/dev/null 2>&1 || true
codesign --verify --deep --strict "$installed_app"

echo "installed: $installed_app"
open "$installed_app"
echo "已启动。以后从启动台 / Spotlight 打开即可，不用再跑控制台。"
