#!/bin/bash
# M1 回归：一条命令跑完「不需要手机的部分」，可选再跑真机腿。
#
#   bash scripts/m1_regression.sh                      # 只跑静态门禁
#   AR_SERIAL=18271FDF600FL4 bash scripts/m1_regression.sh --device   # 加真机腿
#   AR_SERIAL=... bash scripts/m1_regression.sh --device --allow-destructive \
#       --so-target com.example.app --so-name libfoo.so --apk-dir /tmp/apks --write-target com.example.terminal
#
# 为什么要有这个脚本：阶段文档要求「每次改动跑同一套门禁 + 两台设备回归」，
# 而这条清单曾经只存在于我的记忆里 —— 记性不可复现，脚本可以。破坏性腿默认不跑，
# 必须显式给 --allow-destructive 和靶子，避免顺手把用户手机上的应用卸了。
set -uo pipefail
cd "$(dirname "$0")/.."

DEVICE=0; DESTRUCTIVE=0; AR_SERIAL="${AR_SERIAL:-}"
SO_TARGET=""; SO_NAME=""; APK_DIR=""; WRITE_TARGET=""
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE=1 ;;
    --allow-destructive) DESTRUCTIVE=1 ;;
    --so-target) SO_TARGET="$2"; shift ;;
    --so-name) SO_NAME="$2"; shift ;;
    --apk-dir) APK_DIR="$2"; shift ;;
    --write-target) WRITE_TARGET="$2"; shift ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
  shift
done

fails=0
step() {
  local name="$1"; shift
  printf '\n=== %s\n' "$name"
  if "$@"; then
    echo "  ✓ $name"
  else
    echo "  ✗ $name"
    fails=$((fails + 1))
  fi
}

# ---- 静态门禁（不需要设备，永远先跑）----
step "cargo fmt"            cargo fmt --all --check
step "clippy -D warnings"   cargo clippy --workspace --all-targets -- -D warnings
step "cargo test"           cargo test --workspace
step "i18n 五语对齐"         pnpm i18n:check
step "前端类型检查"           pnpm typecheck
step "eslint"               pnpm lint
step "前端测试"              pnpm test
step "前端构建"              pnpm build
step "空白符"                git diff --check

# 原始 demo 模块必须逐字节未改动（基线提交见 docs 台账；这里以「自基线以来无 diff」判定）
APPLIST_BASELINE="${APPLIST_BASELINE:-3f3c3ec}"
step "applist 原始模块未被改动" \
  git diff --quiet "$APPLIST_BASELINE" -- android-zygisk/applist

# 跨 IPC 的 DTO 键名护栏与 shell 调用点登记表：这两份测试是「文档约定」的机器版，
# 单列出来是因为它们失败时含义很明确：有人改了协议字段或新加了 adb shell。
step "协议 DTO 键名两侧对齐" \
  cargo test -p app-reverse-tools --test ipc_dto_wire_shape -- --quiet
step "adb shell 调用点已登记" \
  cargo test -p app-reverse-tools --test shell_call_sites -- --quiet
step "包写操作不产任务卡" \
  cargo test -p app-reverse-tools --test package_write_route -- --quiet

if [ "$DEVICE" -eq 1 ]; then
  if [ -z "$AR_SERIAL" ]; then
    echo "  ✗ --device 需要 AR_SERIAL=<serial>（adb devices 里那个）"
    fails=$((fails + 1))
  else
    export AR4_TEST_SERIAL="$AR_SERIAL" AR6_TEST_SERIAL="$AR_SERIAL" AR7_TEST_SERIAL="$AR_SERIAL"
    export AR8_TEST_SERIAL="$AR_SERIAL" AR9_TEST_SERIAL="$AR_SERIAL" APPLIST_TEST_SERIAL="$AR_SERIAL"
    step "Agent 产物已重编译" bash scripts/build_android_agent.sh aarch64
    # 真机腿串行跑：并行会在同一个 Agent 会话上互相踩踏
    LEGS=(cargo test --workspace -- --ignored --test-threads=1)
    SKIP=(--skip real_agent_package_uninstall)
    if [ "$DESTRUCTIVE" -eq 1 ]; then
      [ -n "$SO_TARGET" ] && [ -n "$SO_NAME" ] && export AR84_TARGET_PKG="$SO_TARGET" AR84_SO="$SO_NAME" AR84_CONFIRM=yes
      [ -n "$APK_DIR" ] && export AR82_APK_DIR="$APK_DIR" AR82_CONFIRM=yes
      [ -n "$WRITE_TARGET" ] && export AR8_WRITE_TARGET="$WRITE_TARGET"
    else
      # 没给授权就不碰：卸载腿、SO 替换腿、安装腿都是会改设备状态的
      SKIP+=(--skip real_agent_replace_native_library --skip real_adb_install)
    fi
    step "真机腿（串行）" "${LEGS[@]}" "${SKIP[@]}"
    # 收尾：转发与常驻进程留给下一次自己起，别在手机上留垃圾
    adb -s "$AR_SERIAL" forward --remove-all || true
    adb -s "$AR_SERIAL" shell "pkill -f app_reverse_tools_agent" || true
  fi
fi

printf '\n===== 结果 =====\n'
if [ "$fails" -eq 0 ]; then
  echo "全部通过"
else
  echo "$fails 步失败（上面每个 ✗ 都有名字，按名字去修，别改断言让它闭嘴）"
fi
exit "$fails"
