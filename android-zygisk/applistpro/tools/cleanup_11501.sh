#!/system/bin/sh
# AR10.5 故障注入前的清场：把 applistpro 的 companion 停掉，好让 11501 空出来
# 交给电脑上的假模块（配合 `adb reverse tcp:11501 tcp:11501`）。
#
# 只杀 cmdline 里带 applistpro 的 companion 进程，别的模块（applist/zygisk_gadget/
# playintegrityfix…）一律不碰。真 companion 被杀掉后**不会自愈**（它不是服务，是 zygote
# fork 出来的常驻子进程），所以实验做完必须 `adb reboot` 把它请回来——这一点踩过两次，
# 别把它当成"模块坏了"。
#
# 用法：
#   adb push 本文件 /data/local/tmp/cleanup_11501.sh
#   adb shell su -c 'sh /data/local/tmp/cleanup_11501.sh'
#
for pat in fake_v2b.sh fake_v2.sh; do
  pkill -9 -f "$pat" 2>/dev/null
done
for p in $(pgrep -f "nc -4 -L"); do
  kill -9 "$p" 2>/dev/null
done
for p in $(pgrep -f zn-zygisk-companion64); do
  if grep -qa applistpro "/proc/$p/cmdline" 2>/dev/null; then kill -9 "$p" 2>/dev/null; fi
done
sleep 2
if netstat -tln 2>/dev/null | grep -q ':11501'; then
  echo STILL_BOUND
  netstat -tlnp 2>/dev/null | grep ':11501'
else
  echo PORT_FREE
fi
