import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { taskApi } from "@/api/task";
import { hookApi, type JsFileDto } from "@/api/hook";
import { useI18n } from "@/i18n";
import { SettingsPanel } from "./SettingsPanel";
import { ScriptList } from "./ScriptList";
import { FridaConsole } from "./FridaConsole";
import type { FridaLaunchRequest } from "@/features/terminal/TerminalPanel";
import { fridaSessionLabel, type FridaSettings, type SessionInfo } from "./types";
import { remoteSpec } from "./types";

/**
 * 右侧那块屏的真终端实现（xterm + 本机 PTY）。
 *
 * 懒加载是**有意的**：xterm 家族 min 后约 823 kB，不该在软件一打开就进主块
 * （第八十三轮实测：主 JS 1,275.87 → 751.94 kB）。放在这里而不是单独子标签，
 * 是因为它就是 frida 控制台本身，不是另一个地方。
 */
const TerminalPanel = lazy(() =>
  import("@/features/terminal/TerminalPanel").then((m) => ({ default: m.TerminalPanel })),
);

/**
 * C 区是**两页**：
 * - `terminal`：那块真终端屏——➕ 开本机会话，点左侧脚本 = 在这页开一个官方 frida REPL 会话
 * - `tracks`：我们自己的 NDJSON 结构化视图（原三轨 + JSON 树）
 *
 * 「设备 shell」**不在这一页**（第八十八轮定的位置）：它是**设备页的 Shell 子标签**，
 * 那里连的是她在设备页选中的那台，也自有的一张会话表。这一页只管本机 shell 与官方 REPL。
 *
 * 切页只改 `hidden`，**绝不卸载**：卸载等于 disposeAll，会把她正在跑的会话连进程一起收掉。
 */
type ConsoleView = "terminal" | "tracks";

const DEFAULT_SETTINGS: FridaSettings = {
  deviceSerial: null,
  connMode: "usb",
  port: "27042",
  runMode: "attach",
  target: "",
};

/**
 * Frida 会话工作台（P10）：3×3 网格——A 设置(1/9)、B 脚本(2/9)、C 控制台(右 2/3 通高)。
 * 会话 = TaskService 任务（kind="frida"）：停止/回放/取消全部复用任务链路。
 */
export function FridaPage() {
  const { t } = useI18n();
  const [settings, setSettings] = useState<FridaSettings>(DEFAULT_SETTINGS);
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [focusScriptsTick, setFocusScriptsTick] = useState(0);
  const [view, setView] = useState<ConsoleView>("terminal");
  const [launch, setLaunch] = useState<FridaLaunchRequest | null>(null);
  const nonceRef = useRef(0);

  const onChange = useCallback(
    (patch: Partial<FridaSettings>) => setSettings((s) => ({ ...s, ...patch })),
    [],
  );

  const start = useCallback(
    async (script: JsFileDto) => {
      /**
       * 默认视图下，点脚本 = 在右侧真终端里起**官方 frida REPL**（路 B）。
       * 参数的形状、环境要求、"没脚本不起会话"都在 `lib/fridaSession`，这里只负责把
       * 脚本绝对路径（B 区给的 `path`）和 A 区当时的设置快照交出去——设置快照这点很重要：
       * 会话起来之后她再改目标，不该把已经跑着的会话搅了。
       */
      if (view !== "tracks") {
        nonceRef.current += 1;
        setLaunch({ scriptPath: script.path, settings: { ...settings }, nonce: nonceRef.current });
        return;
      }
      if (session) return;
      setError(null);
      try {
        const taskId = await hookApi.sessionStart({
          serial: settings.connMode === "usb" ? settings.deviceSerial : null,
          remote: remoteSpec(settings) ?? null,
          spawn: settings.runMode === "spawn",
          // attach 时留空是有意义的：等价 `frida -UF` 的 -F，附加设备当前前台应用。
          // 语义判定在宿主侧 resolve_target 一处做，这里不重复推断也不报错。
          target: settings.target.trim(),
          script: script.name,
        });
        const mode = settings.runMode;
        setSession({
          taskId,
          name: fridaSessionLabel(mode, settings.target, script.name),
          startedAt: Date.now(),
          script: script.name,
          settings: { ...settings },
        });
      } catch (e) {
        setError(String((e as Error)?.message ?? e));
      }
    },
    [session, settings, view],
  );

  const stop = useCallback(() => {
    if (session) void taskApi.cancel(session.taskId).catch((e) => setError(String((e as Error)?.message ?? e)));
  }, [session]);

  const onStopped = useCallback((taskId: string) => {
    setSession((s) => (s && s.taskId === taskId ? null : s));
  }, []);

  // 启动按钮（A 区）语义 = 高亮提示去 B 区选脚本（§3）：短暂闪一下脚本区
  const [flashScripts, setFlashScripts] = useState(false);
  useEffect(() => {
    if (!focusScriptsTick) return;
    setFlashScripts(true);
    const timer = setTimeout(() => setFlashScripts(false), 1600);
    return () => clearTimeout(timer);
  }, [focusScriptsTick]);

  // ⚠️ 显式行列定位（设计 §2 的 3×3 网格）：不能依赖 grid 自动流——
  // row 流下 B(row-span-2)/C(col-span-2 row-span-3) 会顺着行游标摆到顶行右侧，
  // 还把隐式第 4 列撑出来，布局与设计完全错位（首轮实现踩过的坑）。
  return (
    <div className="grid h-full min-h-0 grid-cols-3 grid-rows-3 gap-3">
      {/* A 设置区：左上 1/9（1/3 宽 × 1/3 高） */}
      <div className="col-start-1 row-start-1 min-h-0 min-w-0">
        <SettingsPanel
          settings={settings}
          onChange={onChange}
          running={!!session}
          onGotoScripts={() => setFocusScriptsTick((n) => n + 1)}
          onStop={stop}
        />
      </div>
      {/* B 脚本区：左下 2/9（1/3 宽 × 2/3 高） */}
      <div
        className={`col-start-1 row-start-2 row-span-2 min-h-0 min-w-0 rounded-lg transition-shadow ${
          flashScripts ? "ring-2 ring-sky-500/70" : ""
        }`}
      >
        <ScriptList running={!!session} onStart={(s) => void start(s)} />
      </div>
      {/* C 会话控制台：右侧 2/3，通高。两个视图都一直挂着，切换只改 hidden（卸载=杀会话） */}
      <div className="col-start-2 row-start-1 col-span-2 row-span-3 flex min-h-0 min-w-0 flex-col gap-2">
        <div className="flex shrink-0 items-center gap-1 text-10px" role="group" aria-label={t("hook.frida.consoleView")}>
          {(["terminal", "tracks"] as const).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => setView(key)}
              aria-pressed={view === key}
              className={`rounded px-2 py-0.5 ${
                view === key
                  ? // 与 A 区分段选择同一个选中态语言：选中 = 主色
                    "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-muted/50"
              }`}
              data-testid={`frida-view-${key}`}
            >
              {key === "terminal" ? t("hook.frida.viewTerminal") : t("hook.frida.viewTracks")}
            </button>
          ))}
        </div>
        {/* 「终端」那页：本机 shell + 点脚本起的官方 REPL。切页绝不重载面板（重载 = 杀会话） */}
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
        <div className="min-h-0 flex-1" hidden={view !== "tracks"} data-testid="frida-tracks-view">
          {session ? (
            <FridaConsole session={session} onStopped={onStopped} />
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-2 rounded-xl border border-border/70 bg-card shadow-card/50 text-xs text-muted-foreground">
              <p className="max-w-xs text-center leading-relaxed">{t("hook.frida.consoleIdle")}</p>
              {error && (
                <p className="max-w-md break-all rounded border border-destructive/50 bg-destructive/10 px-2 py-1 text-center text-destructive">
                  {error}
                </p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
