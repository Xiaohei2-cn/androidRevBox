# crypto-base64（示例插件）

演示如何用 `at-plugin-sdk` 实现 host 的 **C ABI v1** 插件契约。P4 阶段作为
加载器与生命周期的验证载体，P5 算法中心沿用同一模式扩展。

## payload 协议

请求 / 响应均为 UTF-8 JSON（响应也通过 `at_plugin_call` 的输出缓冲返回）：

```jsonc
// 请求
{ "op": "encode" | "decode", "data": "<原文或 base64>" }
// 成功
{ "ok": true,  "data": "<结果>" }
// 失败（业务错误走 payload，C 返回值恒 0）
{ "ok": false, "code": 1|2|3, "err": "..." }
```

错误码：`1` 非法 JSON；`2` 未知 op；`3` base64 解码失败。

## 构建（三平台命名 = manifest.entry 键）

```bash
# macOS (arm64)
cargo build -p plugin-crypto-base64 --release
mkdir -p dist/macos-arm64 && cp ../../target/release/libcrypto_base64.dylib dist/macos-arm64/
# Windows (x64)  —— 在 Windows 机器或交叉环境下
#   产物 crypto_base64.dll → dist/windows-x64/
# Linux (x64)
#   产物 libcrypto_base64.so → dist/linux-x64/
```

> 产物文件名由 crate `[[lib]] name` 决定（`crypto_base64` → 平台前/后缀自动加）。
> `dist/` 目录整体即为可分发插件目录（含 `manifest.json`）。

## 部署与测试

```bash
mkdir -p "$APP_DATA/plugins/crypto-base64"          # APP_DATA=~/Library/Application Support/com.appreversetools.app (macOS)
cp -r manifest.json dist/macos-arm64 "$APP_DATA/plugins/crypto-base64/"
# 重启应用或插件中心刷新 → 列表出现 Base64 → plugins_call 验证编解码
cargo test -p plugin-crypto-base64                   # 纯逻辑单测（CI 跑）
```

## ABI 契约（务必遵守）

- 五个导出符号与所有权规则见 [`plugin-sdk/include/plugin_api.h`](../../plugin-sdk/include/plugin_api.h)：
  输出由插件分配、仅经 `at_plugin_free` 归还；host 串行化所有调用。
- 本插件只使用 SDK 的 `export_plugin!`，不手写 extern "C"。
