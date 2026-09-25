import { useCallback, useEffect, useState } from "react";
import { taskApi } from "@/api/task";
import { hookApi, type JsFileDto } from "@/api/hook";
import { useI18n } from "@/i18n";
import { SettingsPanel } from "./SettingsPanel";
import { ScriptList } from "./ScriptList";
import { FridaConsole } from "./FridaConsole";
import { fridaSessionLabel, type FridaSettings, type SessionInfo } from "./types";
import { remoteSpec } from "./types";

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

  const onChange = useCallback(
    (patch: Partial<FridaSettings>) => setSettings((s) => ({ ...s, ...patch })),
    [],
  );

  const start = useCallback(
    async (script: JsFileDto) => {
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
    [session, settings],
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
      {/* C 会话控制台：右侧 2/3，通高 */}
      <div className="col-start-2 row-start-1 col-span-2 row-span-3 min-h-0 min-w-0">
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
  );
}
