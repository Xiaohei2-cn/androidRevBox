#!/bin/bash
# 一键构建 applist Zygisk 模块 zip
# 依赖: Android NDK (ndk-build), Android SDK build-tools (d8), JDK (javac)
set -e
cd "$(dirname "$0")"

SDK="$HOME/Library/Android/sdk"
NDK="$SDK/ndk/28.2.13676358"
BT="$SDK/build-tools/36.1.0"
PLATFORM="$SDK/platforms/android-36.1/android.jar"

echo "==> 1/4 编译 C++ (ndk-build, 在 jni 的父目录执行)"
"$NDK/ndk-build" -j8
mkdir -p pkg/zygisk
cp libs/arm64-v8a/libapplist.so pkg/zygisk/arm64-v8a.so

echo "==> 2/4 编译 Java helper -> dex"
rm -rf helper/build && mkdir -p helper/build
javac --release 8 -classpath "$PLATFORM" -d helper/build helper/src/demo/applist/Helper.java
"$BT/d8" --min-api 26 --lib "$PLATFORM" --output helper/build helper/build/demo/applist/Helper.class
cp helper/build/classes.dex pkg/helper.dex

echo "==> 3/4 打包 module.zip"
chmod 755 pkg/META-INF/com/google/android/update-binary
(cd pkg && zip -qr ../applist.zip module.prop zygisk helper.dex META-INF)

echo "==> 4/4 完成: $(pwd)/applist.zip"
