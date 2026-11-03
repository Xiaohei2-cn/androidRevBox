#!/usr/bin/env bash
# 发版入口：同步四处版本号 → 提交 → 打 tag。
# 推 tag 会触发 .github/workflows/release.yml 出 macOS 安装包并建 Release。
#
#   bash scripts/release.sh 0.2.4          # 只提交 + 打 tag，不推
#   bash scripts/release.sh 0.2.4 --push   # 同时推 main 和 tag
set -euo pipefail

usage() {
  echo "usage: $0 <version> [--push]" >&2
  echo "  version  形如 0.2.4（不带 v 前缀）" >&2
}

[[ $# -ge 1 && $# -le 2 ]] || { usage; exit 2; }
version="$1"
push=false
[[ "${2:-}" == "--push" ]] && push=true
[[ "${2:-}" == "" || "${2:-}" == "--push" ]] || { usage; exit 2; }

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: version 必须是 x.y.z（不带 v 前缀）" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_root="$(cd "$script_dir/.." && pwd)"
cd "$workspace_root"

command -v node >/dev/null 2>&1 || { echo "error: 需要 node" >&2; exit 1; }
command -v git >/dev/null 2>&1 || { echo "error: 需要 git" >&2; exit 1; }

tag="v$version"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "error: tag $tag 已存在" >&2
  exit 1
fi

if [[ -n "$(git status --porcelain -- ':!docs' ':!*.md')" ]]; then
  echo "warning: 工作区还有其他未提交改动，release 提交只会带上版本相关文件" >&2
fi

echo "==> 同步版本到 $version"
node -e '
const fs = require("fs");
const [, , file, version] = process.argv;
const raw = fs.readFileSync(file, "utf8");
const data = JSON.parse(raw);
data.version = version;
let out = JSON.stringify(data, null, 2);
if (raw.endsWith("\n")) out += "\n";
fs.writeFileSync(file, out);
' package.json "$version"
node -e '
const fs = require("fs");
const [, , file, version] = process.argv;
const raw = fs.readFileSync(file, "utf8");
const data = JSON.parse(raw);
data.version = version;
let out = JSON.stringify(data, null, 2);
if (raw.endsWith("\n")) out += "\n";
fs.writeFileSync(file, out);
' src-tauri/tauri.conf.json "$version"

perl -0pi -e 's/(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")/${1}'"$version"'${2}/s' Cargo.toml
perl -0pi -e 's/(\[package\][^\[]*?\nversion = ")[^"]+(")/${1}'"$version"'${2}/s' src-tauri/Cargo.toml

echo "==> 刷新 Cargo.lock"
if ! cargo metadata --format-version 1 --offline >/dev/null 2>&1; then
  echo "warning: cargo metadata 离线刷新失败；下次构建会自己更新 Cargo.lock" >&2
fi

git add package.json src-tauri/tauri.conf.json Cargo.toml src-tauri/Cargo.toml
[[ -f Cargo.lock ]] && git add Cargo.lock

# 只提交版本相关文件，避免把工作区其它改动一起带进 release 提交
git commit -m "release: $tag" -- \
  package.json src-tauri/tauri.conf.json Cargo.toml src-tauri/Cargo.toml Cargo.lock
git tag -a "$tag" -m "release: $tag"

echo
echo "已提交并打 tag ${tag}。"
if [[ "$push" == true ]]; then
  git push origin HEAD
  git push origin "$tag"
  echo "已推送；GitHub Actions 会开始构建 macOS 安装包。"
else
  echo "下一步：git push origin HEAD && git push origin $tag"
  echo "（或重跑：bash scripts/release.sh $version --push，此时会因 tag 已存在而失败，请直接执行上面的 push 命令）"
fi
