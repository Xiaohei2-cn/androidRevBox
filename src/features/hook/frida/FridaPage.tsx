import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { configApi } from "@/api/config";
import { useI18n } from "@/i18n";
import { SettingsPanel } from "./SettingsPanel";
import { ScriptList } from "./ScriptList";
import { KEY_HOOK_WORKDIR } from "./workdir";
import type { FridaLaunchRequest } from "@/features/terminal/TerminalPanel";
import type { FridaSettings } from "./types";
import type { OpenRequest } from "./ide/ScriptIde";

/**
 * 右侧那块屏的真终端实现（xterm + 本机 PTY）。
 *
 * 懒加载是**有意的**：xterm 家族 min 后约 823 kB，不该在软件一打开就进主块。
 */
const TerminalPanel = lazy(() =>
  import("@/features/terminal/TerminalPanel").then((m) => ({ default: m.TerminalPanel })),
);

/**
 * 脚本编辑器（Monaco）。懒加载的理由与终端同款、量级更大：编辑器主体 + TS 语言服务
 * worker 合计 10 MB 上下（解压后），只有真的打开过 IDE 才该付这笔钱。
 */
const ScriptIde = lazy(() =>
  import("./ide/ScriptIde").then((m) => ({ default: m.ScriptIde })),
);

/**
 * C 区是**两页**（终端在前——点进 frida 页先看到的是会话那块屏）：
 * - `terminal`：真终端屏（**默认**）——点某行的「▶」或 IDE 的「运行」= 在这页起官方 frida REPL
 * - `ide`：脚本编辑器——B 区点一行 = 在这页打开，改完保存/运行
 *
 * 「设备 shell」**不在这一页**（第八十八轮定的位置）：它是**设备页的 Shell 子标签**。
 * 切页只改 `hidden`，**绝不卸载**：卸载等于 disposeAll，会把她正在跑的会话连进程一起收掉。
 */
type ConsoleView = "terminal" | "ide";

const DEFAULT_SETTINGS: FridaSettings = {
  deviceSerial: null,
  connMode: "usb",
  port: "27042",
  runMode: "attach",
  target: "",
};

/**
 * Frida 工作台（P10）：3×3 网格——A 设置(1/9)、B 脚本(2/9)、C 编辑器/终端(右 2/3 通高)。
 *
 * 会话 = TaskService 任务（kind="frida"）已经是过去式：结构化终端与 runner 整体退役后，
 * 起会话统一走真终端里的官方 REPL（PTY），任务中心不再出现 frida 任务卡——
 * 会话在哪块屏上跑，就在哪块屏上停（终端自己的标签页就是它的生命周期）。
 */
export function FridaPage() {
  const { t } = useI18n();
  const [settings, setSettings] = useState<FridaSettings>(DEFAULT_SETTINGS);
  const [view, setView] = useState<ConsoleView>("terminal");
  const [launch, setLaunch] = useState<FridaLaunchRequest | null>(null);
  const nonceRef = useRef(0);

  /** 工作目录状态归本页持有：B 区（列表）与 C 区（编辑器读写）必须同一份真相 */
  const [workdir, setWorkdir] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void configApi
      .get(KEY_HOOK_WORKDIR, "")
      .then((v) => alive && setWorkdir(v.trim()))
      .catch(() => alive && setWorkdir(""));
    return () => {
      alive = false;
    };
  }, []);

  /** 编辑器要打开的脚本（B 区单击一行 / 新建后的自动打开都推这里） */
  const [openRequest, setOpenRequest] = useState<OpenRequest | null>(null);
  const openNonce = useRef(0);
  /** B 区列表的刷新信号（编辑器保存/新建/删除后推一步） */
  const [listTick, setListTick] = useState(0);
  /** 编辑器里有未保存改动（B 区行上要标出来；用户不许在不知情时丢掉改动） */
  const [dirtyNames, setDirtyNames] = useState<ReadonlySet<string>>(new Set());
  /** 「运行」被点了但当前视图在 IDE：切到终端页 */
  const [pendingRun, setPendingRun] = useState<string | null>(null);

  const onChange = useCallback(
    (patch: Partial<FridaSettings>) => setSettings((s) => ({ ...s, ...patch })),
    [],
  );

  const openInIde = useCallback((name: string) => {
    openNonce.current += 1;
    setOpenRequest({ name, nonce: openNonce.current });
    // 她点的是"打开"——即便当前停在终端视图，也要把编辑器切到眼前，
    // 否则请求发出去了、屏幕上还是终端，看起来就是"点了没反应"。
    setView("ide");
  }, []);

  /**
   * 起官方 REPL 会话（路 B）：
   * - 从 B 区行的「▶」来：scriptPath = 工作目录 + 名字
   * - 从 IDE 的「运行」来：同上（编辑器保存过的当前文件）
   * 参数的形状、环境要求、"没脚本不起会话"都在 `lib/fridaSession`，这里只把
   * 脚本绝对路径与 A 区设置快照交出去——设置快照很重要：会话起来之后她再改目标，
   * 不该把已经跑着的会话搅了。
   */
  const start = useCallback(
    (name: string) => {
      if (!workdir) return;
      const scriptPath = name.startsWith("/")
        ? name
        : `${workdir.replace(/[\\/]+$/, "")}/${name}`;
      nonceRef.current += 1;
      setLaunch({ scriptPath, settings: { ...settings }, nonce: nonceRef.current });
      setView("terminal");
    },
    [settings, workdir],
  );

  /** IDE 的「运行」：先切到终端页，再等 launch 挂上（切页只改 hidden，两个都常驻） */
  useEffect(() => {
    if (!pendingRun) return;
    start(pendingRun);
    setPendingRun(null);
  }, [pendingRun, start]);

  // ⚠️ 显式行列定位（设计 §2 的网格）：不能依赖 grid 自动流——
  // row 流下 B/C 会顺着行游标摆到顶行右侧，还把隐式第 4 列撑出来（首轮实现踩过的坑）。
  //
  // 行高 auto + 1fr：A 区「会话设置」按内容取高；剩下都归 B 区（它自己滚）。
  return (
    <div className="grid h-full min-h-0 grid-cols-3 grid-rows-[auto_minmax(0,1fr)] gap-3">
      {/* A 设置区：左上，宽度 1/3，高度由内容决定 */}
      <div className="col-start-1 row-start-1 min-w-0">
        <SettingsPanel settings={settings} onChange={onChange} />
      </div>
      {/* B 脚本区：左下，吃掉 A 之外的全部高度（列表长，滚动条归它） */}
      <div className="col-start-1 row-start-2 min-h-0 min-w-0">
        <ScriptList
          workdir={workdir}
          onWorkdirChange={setWorkdir}
          onStart={(f) => start(f.name)}
          onOpen={(f) => openInIde(f.name)}
          refreshSignal={listTick}
          dirtyNames={dirtyNames}
        />
      </div>
      {/* C 编辑器/终端：右侧 2/3，通高。两页都一直挂着，切换只改 hidden（卸载=杀会话） */}
      <div className="col-start-2 row-start-1 col-span-2 row-span-2 flex min-h-0 min-w-0 flex-col gap-2">
        <div
          className="flex shrink-0 items-center gap-1 text-10px"
          role="group"
          aria-label={t("hook.frida.consoleView")}
        >
          {/* 终端在前、编辑器在后：打开 frida 页先看到会话那块屏（用户第九十七轮定的顺序） */}
          {(["terminal", "ide"] as const).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => setView(key)}
              aria-pressed={view === key}
              className={`rounded px-2 py-0.5 ${
                view === key
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-muted/50"
              }`}
              data-testid={`frida-view-${key}`}
            >
              {key === "terminal" ? t("hook.frida.viewTerminal") : t("hook.frida.viewIde")}
            </button>
          ))}
        </div>

        {/* 终端那页：本机 shell + 点脚本/运行起的官方 REPL。切页绝不重载面板（重载=杀会话） */}
        <div className="min-h-0 flex-1" hidden={view !== "terminal"} data-testid="frida-terminal-view">
          <Suspense
            fallback={
              <div className="p-3 text-10px text-muted-foreground" data-testid="terminal-loading">
                {t("hook.terminal.loading")}
              </div>
            }
          >
            <TerminalPanel fridaLaunch={launch} visible={view === "terminal"} />
          </Suspense>
        </div>

        {/* IDE 那页：脚本编辑器（B 区单击一行 = 在这里打开）。
            顺序在终端之后 = DOM 里也靠后：打开 frida 页时先入眼的是上面那块终端屏 */}
        <div className="min-h-0 flex-1" hidden={view !== "ide"} data-testid="frida-ide-view">
          <Suspense
            fallback={
              <div className="p-3 text-10px text-muted-foreground" data-testid="ide-loading">
                {t("hook.frida.ide.loading")}
              </div>
            }
          >
            <ScriptIde
              workdir={workdir}
              onWorkdirChange={setWorkdir}
              request={openRequest}
              onRun={(name) => setPendingRun(name)}
              onChanged={() => setListTick((n) => n + 1)}
              // 编辑器按文件维护脏集合（切文件不丢改动），这里原样转给 B 区画 ●
              onDirtyChange={(dirtySet) => setDirtyNames(dirtySet)}
            />
          </Suspense>
        </div>
      </div>
    </div>
  );
}