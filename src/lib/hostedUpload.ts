/**
 * 二进制托管的"上传/拖入"规划层（纯函数，可无设备单测）。
 *
 * 这里只做**决定**：拖进来/选中的宿主路径，哪些能进托管目录、落到哪个远端路径、
 * 是不是覆盖。真正的传输是 `adb push`（TaskService 长任务），赋执行权限是 Agent 的
 * `hosted.chmod`（只补 0o111，幂等）。
 */

/** 托管目录：与 Desktop `adapters::adb::HOSTED_DIR`、Agent `hosted::HOSTED_DIR` 同值 */
export const HOSTED_DIR = "/data/local/tmp";

/** 与 Rust `is_safe_hosted_name` 同形，另加 Agent `hosted_path` 的 128 长度上限 */
export function isSafeHostedName(name: string): boolean {
  return (
    name.length > 0 &&
    name.length <= 128 &&
    !name.startsWith(".") &&
    /^[A-Za-z0-9_.-]+$/.test(name)
  );
}

/**
 * 取文件名。访达给 `/Users/.../x`，Windows 给 `C:\...\x`，两种分隔符都要认：
 * 只按 `/` 切会让 Windows 拖入时拿整条路径去当文件名，白拦一道。
 */
export function hostedBaseName(path: string): string {
  const trimmed = path.trimEnd();
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const name = cut >= 0 ? trimmed.slice(cut + 1) : trimmed;
  return name.replace(/[;]$/, ""); // adb 拖入偶发的结尾分号（访达"复制路径"粘贴）
}

export interface UploadItem {
  /** 宿主绝对路径 */
  local: string;
  /** 设备侧目标路径 */
  remote: string;
  /** 托管目录里的文件名（也是列表里那一行的 key） */
  name: string;
  /** 同名文件已在托管目录里 → adb push 会覆盖它 */
  overwrite: boolean;
  /** 同名进程正在运行 → 覆盖只影响下次启动，这话必须说清 */
  running: boolean;
}

export interface UploadReject {
  path: string;
  reason: "no-name" | "bad-name" | "duplicate";
}

export interface UploadPlan {
  accepted: UploadItem[];
  rejected: UploadReject[];
}

/**
 * 把一批宿主路径规划成"要推的文件"。
 *
 * 拒绝的必须带原因回给界面：**静默丢文件**是这类入口最坏的实现方式
 * （用户只会觉得"拖了没反应"）。名字不合白名单的文件推上去也不会出现在托管列表里
 * （Agent 扫目录时同样按白名单过滤），所以在这里就拦、并说清楚为什么。
 */
export function planHostedUpload(
  paths: readonly string[],
  known: { names: Set<string>; runningNames: Set<string> },
): UploadPlan {
  const accepted: UploadItem[] = [];
  const rejected: UploadReject[] = [];
  const seen = new Set<string>();
  for (const raw of paths) {
    const local = raw.trim();
    if (!local) continue;
    const name = hostedBaseName(local);
    if (!name) {
      rejected.push({ path: raw, reason: "no-name" });
      continue;
    }
    if (!isSafeHostedName(name)) {
      rejected.push({ path: raw, reason: "bad-name" });
      continue;
    }
    if (seen.has(name)) {
      // 同一批里重名：adb push 到同一个目标只会互相覆盖，第二份直接拒并说明
      rejected.push({ path: raw, reason: "duplicate" });
      continue;
    }
    seen.add(name);
    accepted.push({
      local,
      remote: `${HOSTED_DIR}/${name}`,
      name,
      overwrite: known.names.has(name),
      running: known.runningNames.has(name),
    });
  }
  return { accepted, rejected };
}

/** 一句人话说明为什么被拒（界面直接显示，不甩"内部错误"） */
export function rejectReasonText(reason: UploadReject["reason"]): string {
  switch (reason) {
    case "no-name":
      return "拿不到文件名（拖入的路径以分隔符结尾？只支持文件，不支持目录）";
    case "bad-name":
      return "文件名不符合托管白名单：只允许字母数字与 _ . -，不能以 . 开头，长度 ≤128";
    case "duplicate":
      return "同一批里有重名文件，只推第一个（否则会互相覆盖）";
  }
}
