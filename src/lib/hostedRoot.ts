/**
 * 二进制托管的 Root (su) 偏好（第六十三轮：用户要求「默认给 root 权限」）。
 *
 * 口径是"**默认想开，但要现场证明**"：没手动选过的设备，进页面就探一次 su，
 * 探得过就以 root 执行；探不过就保持普通执行**并说明原因**——安静地把 root 当成已生效，
 * 比不开更糟（用户会以为 frida-server 是 root 起的，而 shell 起的注入不了别人）。
 *
 * 用户的手动选择按设备记住：关掉过就不再自动开（非 root 的机器不该每次被探测、被提示）。
 */

export type RootPref = "unset" | "on" | "off";

export const rootPrefKey = (serial: string): string => `adb.binary.root.${serial}`;

export function loadRootPref(serial: string | null | undefined): RootPref {
  if (!serial) return "unset";
  try {
    const raw = localStorage.getItem(rootPrefKey(serial));
    if (raw === "0") return "off";
    if (raw === "1") return "on";
  } catch {
    // 存储不可用：按"没选过"处理，走默认探测
  }
  return "unset";
}

/** 记不记"没选过"要分开：unset 与 on 都要去探测，只有 off 是不再尝试 */
export function shouldTryRoot(pref: RootPref): boolean {
  return pref !== "off";
}

export function saveRootPref(serial: string | null | undefined, on: boolean): void {
  if (!serial) return;
  try {
    localStorage.setItem(rootPrefKey(serial), on ? "1" : "0");
  } catch {
    // 写不进去只影响下次仍走默认探测，不影响本次
  }
}
