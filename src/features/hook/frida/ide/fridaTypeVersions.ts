/**
 * frida 类型库的**版本解析**（智能提示的"提示内容"跟着你选定的 frida 版本走）。
 *
 * 为什么需要它：`@types/frida-gum` / `frida-java-bridge` 是**按 frida 主线发版**的，
 * 版本对不上时提示会是错的用法（例如 frida 16 的脚本被 19.x 类型库按新 API 提示）。
 * 内置一份是"兜底"，但它必须能被选定的环境覆盖——否则用户换 frida 版本就没法用。
 *
 * 为什么不在 Python/Node 环境里找类型文件：**那里没有**。实测 frida 的 PyPI 包里只有
 * `_frida.abi3.so` 与 `.pyi`（Python 自身的类型，与 JS API 无关），`frida-tools` 零个
 * `.d.ts`；类型库只存在于 npm 生态。所以只能"按版本从 npm 取"。
 *
 * 取法：从 jsDelivr CDN 拉对应版本的 `.d.ts` 文本（纯文本、无依赖、可缓存），
 * 失败则回退到内置版本（保证离线/内网也能用，只是提示可能滞后）。
 */

/** 一份类型库的坐标与正文 */
export interface FridaTypeLib {
  /** 展示用：`@types/frida-gum@19.12.0` */
  label: string;
  /** 注入 Monaco 用的 d.ts 正文 */
  content: string;
  /** 来源：`env` = 跟着选定环境解析到的版本；`bundled` = 内置兜底 */
  source: "env" | "bundled";
}

export interface ResolvedFridaTypes {
  gum: FridaTypeLib;
  javaBridge: FridaTypeLib;
  /** 解析结论的一句话说明（设置页/状态条显示给人看） */
  note: string;
}

/**
 * frida 主线 → 类型库版本（实测 npm 上的版本与发布日期对应）。
 *
 * 只列有把握的映射；表里没有的主线一律**回退内置**（宁可提示略旧，也不要猜错版本
 * 给出错误 API）。表里的值都是该主线上 npm 的最新小版本（实测 2026-10）。
 */
const GUM_BY_FRIDA_MAJOR: Record<number, string> = {
  12: "12.6.0",
  13: "13.1.0",
  14: "14.5.0",
  15: "15.3.0",
  16: "16.5.1",
  17: "17.3.1",
  18: "18.8.2",
  19: "19.12.0",
};

/**
 * frida 主线 → `frida-java-bridge` 版本。**只列 7.x**：实测该包从 7.0.0 起才发布
 * `index.d.ts`，之前的主线在 npm 上根本没有类型文件（CDN 实测 6.3.9 → 404）。
 * 所以 frida ≤16 时 java-bridge 一律回退内置（如实说明），gum 仍按主线取——
 * gum 才是 API 主体，java-bridge 只是 `Java.*` 桥，且 7.x 的桥面与 16/17 用法一致。
 */
const JAVA_BRIDGE_BY_FRIDA_MAJOR: Record<number, string> = {
  17: "7.0.13",
  18: "7.0.13",
  19: "7.0.13",
};

/** 取 frida 版本的主线号（`16.5.7` → 16）。解析不出返回 null。 */
export function fridaMajor(version: string | null | undefined): number | null {
  if (!version) return null;
  const m = /^\s*(\d+)\./.exec(version);
  return m ? Number(m[1]) : null;
}

/** 按 frida 版本算出该用哪两个类型库版本；查不到映射返回 null（调用方回退内置）。 */
export function typesVersionsFor(
  version: string | null | undefined,
): { gum: string; javaBridge: string | null } | null {
  const major = fridaMajor(version);
  if (major === null) return null;
  const gum = GUM_BY_FRIDA_MAJOR[major];
  if (!gum) return null;
  // javaBridge 允许 null：见上表注释（7.x 之前没有发布 d.ts）
  return { gum, javaBridge: JAVA_BRIDGE_BY_FRIDA_MAJOR[major] ?? null };
}

/** jsDelivr 上某个 npm 包某版本的某文件 URL（CDN 直接给纯文本） */
function cdnUrl(pkg: string, version: string, file: string): string {
  return `https://cdn.jsdelivr.net/npm/${pkg}@${version}/${file}`;
}

/** 单次拉取超时：联网拉类型库不该让 IDE 打开变慢，拿不到就走兜底 */
const FETCH_TIMEOUT_MS = 6_000;

async function fetchText(url: string): Promise<string | null> {
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), FETCH_TIMEOUT_MS);
    const res = await fetch(url, { signal: ctrl.signal });
    clearTimeout(timer);
    if (!res.ok) return null;
    const text = await res.text();
    // 防呆：CDN 出错时可能回一段 HTML/短文本，别把垃圾注进语言服务
    return text.length > 1000 ? text : null;
  } catch {
    return null;
  }
}

/**
 * 解析该用哪两份类型库，并把正文取回来。
 *
 * `bundled` 是打进产物的内置版本（构建期 `?raw` 导入），任何一步失败都回退到它——
 * 智能提示**绝不会因为没网而消失**，只会退到内置版本并如实说明。
 */
export async function resolveFridaTypes(
  fridaVersion: string | null | undefined,
  bundled: { gum: string; javaBridge: string | null },
  cache: Map<string, string>,
): Promise<ResolvedFridaTypes> {
  const versions = typesVersionsFor(fridaVersion);
  if (!versions) {
    return {
      gum: { label: "内置版本", content: bundled.gum, source: "bundled" },
      javaBridge: { label: "内置版本", content: bundled.javaBridge ?? "", source: "bundled" },
      note: fridaVersion
        ? `未知的 frida 主线（${fridaVersion}）：用内置类型库`
        : "还没探测到 frida 版本：用内置类型库（配好 Python 环境后自动跟随）",
    };
  }

  const gumKey = `@types/frida-gum@${versions.gum}`;

  const [gum, javaBridge] = await Promise.all([
    loadLib(gumKey, cdnUrl("@types/frida-gum", versions.gum, "index.d.ts"), bundled.gum, cache),
    // javaBridge 版本可能没有（frida ≤16：该包 7.x 前没发 d.ts）——那就直接用内置，
    // 并在说明里如实点出，不假装"跟随了"。
    versions.javaBridge
      ? loadLib(
          `frida-java-bridge@${versions.javaBridge}`,
          cdnUrl("frida-java-bridge", versions.javaBridge, "index.d.ts"),
          bundled.javaBridge ?? "",
          cache,
        )
      : Promise.resolve<FridaTypeLib>({
          label: "内置版本",
          content: bundled.javaBridge ?? "",
          source: "bundled",
        }),
  ]);

  const gumFollows = gum.source === "env";
  const jbFollows = javaBridge.source === "env";
  let note: string;
  if (gumFollows && jbFollows) {
    note = `类型库跟随 frida ${fridaVersion}：${gumKey} + frida-java-bridge@${versions.javaBridge}`;
  } else if (gumFollows && !jbFollows) {
    note = `frida ${fridaVersion}：gum 类型跟随（${gumKey}）；java-bridge 该主线未发布类型，用内置版本`;
  } else {
    note = `frida ${fridaVersion} 的类型库拉取失败，已回退内置版本（提示可能滞后；离线时属正常）`;
  }
  return { gum, javaBridge, note };
}

async function loadLib(
  key: string,
  url: string,
  fallback: string,
  cache: Map<string, string>,
): Promise<FridaTypeLib> {
  const hit = cache.get(key);
  if (hit) return { label: key, content: hit, source: "env" };
  const text = await fetchText(url);
  if (!text) return { label: "内置版本", content: fallback, source: "bundled" };
  cache.set(key, text);
  return { label: key, content: text, source: "env" };
}
