/**
 * Monaco 装载的护栏（为什么要用"读源码"来钉，见下）。
 *
 * 背景是**一次真实的静默失效**：编辑器壳体从 `editor.api` 进（精简 API 工厂，只 6 个
 * import），而补全菜单、折叠、查找、括号配对全是**贡献点（contrib）**，得自己引。
 * 忘了引不会报错——TS worker 照旧算得出补全（实测 `Interceptor.` 回 8 项），
 * 但屏上永远不弹建议控件。这种"算得出、看不见"的失效没法用单元测试抓（jsdom 里
 * 也没有真 Monaco），只能用一条读源码的护栏钉住"这几个 contrib 必须在"。
 *
 * 同时钉住类型库的来源：不许再回到"写死一份内置版本、用户换 frida 就没法用"的老路。
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = process.cwd();
const SETUP = "src/features/hook/frida/ide/monacoSetup.ts";
const VERSIONS = "src/features/hook/frida/ide/fridaTypeVersions.ts";

function read(rel: string): string {
  return readFileSync(path.join(ROOT, rel), "utf8");
}

describe("Monaco 装载：必备 contrib 必须在（缺了会静默失效）", () => {
  const src = read(SETUP);

  it("补全菜单本体（suggestController）被引入", () => {
    // 这就是那次"提示根本没生效"的根因：少了这一行，控件永远不弹，且不报错
    expect(src).toMatch(/editor\/contrib\/suggest\/browser\/suggestController/);
  });

  it("折叠 / 悬停 / 括号配对 / 查找 / 注释 都在", () => {
    for (const contrib of [
      "contrib/folding/browser/folding",
      "contrib/hover/browser/hoverContribution",
      "contrib/bracketMatching/browser/bracketMatching",
      "contrib/find/browser/findController",
      "contrib/comment/browser/comment",
    ]) {
      expect(src, `缺 contrib：${contrib}`).toContain(contrib);
    }
  });

  it("不许退回到裸的 editor.api（那等于把交互功能全丢掉）", () => {
    // editor.api 可以用作类型/工厂，但**必须**同时显式引入 contrib；
    // 这条断言配合上面几条一起，防止有人"为了省体积"又把 contrib 删掉。
    expect(src).toContain("editor/editor.api");
    expect(src).toContain("editor/browser/coreCommands");
  });
});

describe("类型库版本解耦：不许写死", () => {
  const setupSrc = read(SETUP);
  const versionsSrc = read(VERSIONS);

  it("有统一的注入入口（applyFridaTypes），且项目目录优先", () => {
    expect(setupSrc).toMatch(/export async function applyFridaTypes\b/);
    // 解析逻辑单列在 fridaTypeVersions（可单测），不在 setup 里散着
    expect(versionsSrc).toMatch(/export function typesVersionsFor/);
    // 项目目录是**最高优先级**来源：先读它，读不到（用户没填/填错）才回退按版本解析。
    // 这条防的是有人把顺序改回去（把"按 frida 版本"提到前面，用户指定的那份就失效了）。
    const dirIdx = setupSrc.indexOf("hookApi.fridaTypes(dir)");
    const cdnIdx = setupSrc.indexOf("resolveFridaTypes(fridaVersion");
    expect(dirIdx).toBeGreaterThan(0);
    expect(cdnIdx).toBeGreaterThan(0);
    expect(dirIdx).toBeLessThan(cdnIdx);
  });

  /**
   * 类型库变更时必须**重建 TS worker**——这是"状态条说 18.8.2、提示却还是 19.x"
   * 那个事故的根治手段：语言服务把已解析的 lib/ScriptInfo 缓存在 worker 进程里，
   * `addExtraLib` 只推新 lib，**已算过的补全不保证重算**；加上 worker 工厂闭包挂在
   * `globalThis.MonacoEnvironment`（跨 HMR 存活），光改模块变量管不住它。
   */
  it("worker 与类型库不同步时重建（否则提示不会跟着版本走）", () => {
    expect(setupSrc).toMatch(/function recycleTsWorker/);
    expect(setupSrc).toMatch(/w\.terminate\(\)/);
    expect(setupSrc).toMatch(/injectFridaLibs\(libs, rebuildNow\)/);
    expect(setupSrc).toMatch(/injectFridaLibs\(BUNDLED_FRIDA_TYPES, false\)/);
    // 首次注入必须登记指纹
    expect(setupSrc).toMatch(/appliedKey = typeSetKey\(BUNDLED_FRIDA_TYPES\)/);
    /*
     * 判据必须是"**worker 现在这个库里建的吗**"，而不是"这一拍有没有变更"。
     *
     * 事故现场（落盘诊断抓到）：注入的明明是 18.8.2、内容也含 findBaseAddress，
     * 但 `changed=false` → 走"没变，什么都不做" → `recycleTsWorker` 再也不触发，
     * 而 worker 早按内置 19.x 算过并缓存 → 表现为"注入没生效"。
     */
    expect(setupSrc).toMatch(/workerLibKey/);
    expect(setupSrc).toMatch(/const workerInSync = workerLibKey === key/);
    expect(setupSrc).toMatch(/const rebuildNow = !workerInSync && workerRunning/);
  });

  /**
   * `setupMonaco()` 里的内置兜底**不许覆盖已注入的那份**。
   *
   * 真实事故（只在 app 里复现）：ScriptIde 的 effect 先注入了用户的 18.8.2；
   * 用户点开编辑器时 setupMonaco 首次执行，此前无条件注内置 → **把它覆盖回 19.x**，
   * 连指纹也改回去，于是此后 applyFridaTypes 即便再跑也认为"没变"。
   */
  it("setupMonaco 的兜底注入不得覆盖已解析的库（app 里才暴露的时序 bug）", () => {
    // 必须带 appliedKey === null 的守卫
    expect(setupSrc).toMatch(/if \(appliedKey === null\)/);
    // 且守卫必须紧挨着那次兜底注入（同一分支内）
    const guarded = /if \(appliedKey === null\) \{[\s\S]{0,200}injectFridaLibs\(BUNDLED_FRIDA_TYPES, false\)/;
    expect(setupSrc).toMatch(guarded);
  });

  it("Java 垫片按需注入（gum 自带 Java 时不再叠 java-bridge 的声明）", () => {
    // 修的是"同一命名空间声明两次"：gum 18.x 及更早自带 `declare namespace Java`，
    // 那种情况下必须只留 gum 那份。判定标志是 gumHasJava。
    expect(setupSrc).toMatch(/gumHasJava/);
    expect(setupSrc).toMatch(/JAVA_FROM_JB_SHIM/);
    // 条件注入：canBridgeJava 才注（不许无条件注）
    expect(setupSrc).toMatch(/canBridgeJava/);
  });

  it("映射表只列有把握的主线；未知主线回退内置（不猜）", () => {
    // `99` 这种乱版本不该出现在映射表里
    expect(versionsSrc).not.toMatch(/^\s*99:/m);
    // 回退路径必须存在（内置兜底）
    expect(setupSrc).toContain("BUNDLED_FRIDA_TYPES");
  });
});
