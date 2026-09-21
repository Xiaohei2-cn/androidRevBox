#!/bin/bash
# ApplistPro 一键构建（独立于 demo applist 模块，产物与 module id 都不同，可并存）
# 依赖: Android NDK (ndk-build), Android SDK build-tools (d8), JDK (javac)
set -e
cd "$(dirname "$0")"

SDK="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
NDK="$SDK/ndk/28.2.13676358"
BT="$SDK/build-tools/36.1.0"
PLATFORM="$SDK/platforms/android-36.1/android.jar"

echo "==> 1/4 编译 C++ (ndk-build)"
"$NDK/ndk-build" -j8
mkdir -p pkg/zygisk
cp libs/arm64-v8a/libapplistpro.so pkg/zygisk/arm64-v8a.so

echo "==> 2/4 编译 Java helper -> dex"
rm -rf helper/build && mkdir -p helper/build
javac --release 8 -nowarn -classpath "$PLATFORM" -d helper/build helper/src/pro/applist/HelperPro.java
"$BT/d8" --min-api 26 --lib "$PLATFORM" --output helper/build $(find helper/build -name '*.class')
cp helper/build/classes.dex pkg/helper.dex

echo "==> 3/4 打包 applistpro.zip"
chmod 755 pkg/META-INF/com/google/android/update-binary
rm -f applistpro.zip
(cd pkg && zip -qr ../applistpro.zip module.prop zygisk helper.dex META-INF)

echo "==> 4/4 完成: $(pwd)/applistpro.zip"
echo "    安装: adb push applistpro.zip /data/local/tmp && adb shell su -c 'ksud module install /data/local/tmp/applistpro.zip' && adb reboot"
echo "    令牌: /data/adb/applistpro.token (0600 root，模块目录内那份是兼容镜像) —— Agent 读取后经 H 握手使用"
