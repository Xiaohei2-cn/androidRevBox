/**
 * 真终端页（FT0.3 的最小闭环）：本机 PTY + xterm.js。
 *
 * 这一页存在的意义就是"不再自己画终端"。组件里只做四件事：
 * 挂载/卸载、把键盘交给会话控制层、把容器尺寸变化交给控制层、把状态话说清楚。
 * 一个字节的转义处理都不在这里（那是 xterm 的事，见 `src/lib/terminal.ts` 那条铁律）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CircleStop, Play, RotateCcw } from "lucide-react";

import { ptyApi, type PtyExit } from "@/api/pty";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import { createTerminal } from "@/lib/terminal";
import { TerminalSession } from "@/lib/terminalSession";
import "@xterm/xterm/css/xterm.css";

interface SessionState {
  running: boolean;
  exitCode: number | null;
  cols: number;
  rows: number;
}

export function TerminalPage() {
  const { t } = useI18n();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const [program, setProgram] = useState("");
  const [argsText, setArgsText] = useState("");
  const [state, setState] = useState<SessionState>({ running: false, exitCode: null, cols: 0, rows: 0 });
  const [error, setError] = useState<string | null>(null);
  const [renderer, setRenderer] = useState<"webgl" | "dom" | null>(null);
  const [busy, setBusy] = useState(false);

  // 起会话：终端实例挂在 store/refs 上，不随 render 重建（重建 = 丢缓冲区）
  useEffect(() => {
    let alive = true;
    void ptyApi.defaultProgram()
      .then((value) => alive && setProgram((current) => current || value))
      .catch((reason) => alive && setError(String((reason as Error)?.message ?? reason)));
    return () => {
      alive = false;
    };
  }, []);

  const start = useCallback(async () => {
    const host = hostRef.current;
    if (!host || busy) return;
    setBusy(true);
    setError(null);
    try {
      const bundle = createTerminal({ cols: Math.max(20, Math.floor(host.clientWidth / 8) || 80), rows: Math.max(4, Math.floor(host.clientHeight / 16) || 24) });
      setRenderer(bundle.renderer);
      const session = new TerminalSession(bundle, ptyApi, {
        onState: (next) => setState(next),
        onError: (message) => setError(message),
      });
      session.attach();
      sessionRef.current = session;
      // 容器已经在页面上，先量一次再 open 之外补一次（xterm 需要元素在文档里才有尺寸）
      requestAnimationFrame(() => session.fit());
      const args = argsText.trim() ? argsText.trim().split(/\s+/) : [];
      await session.start(program.trim(), args, null);
    } catch (reason) {
      setError(String((reason as Error)?.message ?? reason));
    } finally {
      setBusy(false);
    }
  }, [argsText, busy, program]);

  const stop = useCallback(async () => {
    setBusy(true);
    try {
      await sessionRef.current?.stop();
    } finally {
      setBusy(false);
    }
  }, []);

  const restart = useCallback(async () => {
    setBusy(true);
    sessionRef.current?.dispose();
    sessionRef.current = null;
    setRenderer(null);
    setState({ running: false, exitCode: null, cols: 0, rows: 0 });
    setBusy(false);
    await start();
  }, [start]);

  // 卸载 = 关掉会话：不留活进程（R-08）
  useEffect(
    () => () => {
      sessionRef.current?.dispose();
      sessionRef.current = null;
    },
    [],
  );

  /**
   * 尺寸跟着容器走。tab 被 `hidden` 时量到 0 —— 那时 `fit()` 返回 false 且**什么都不改**，
   * 回到可见时 ResizeObserver 会因为尺寸从 0 变成有值再触发一次（R-03 的处置）。
   */
  useEffect(() => {
    const host = hostRef.current;
    if (!host || typeof ResizeObserver === "undefined") return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        sessionRef.current?.fit();
      });
    });
    observer.observe(host);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, []);

  // 退出通知由控制层接；这里只兜住"界面在会话之外自己关掉"的情况
  useEffect(() => {
    let un: (() => void) | undefined;
    void ptyApi
      .onExit((exit: PtyExit) => {
        if (sessionRef.current?.sessionId !== exit.id) return;
        setState((prev) => ({ ...prev, running: false, exitCode: exit.exitCode }));
      })
      .then((dispose) => {
        un = dispose;
      })
      .catch(() => undefined);
    return () => un?.();
  }, []);

  const statusLine = useMemo(() => {
    if (state.running) return t("hook.terminal.running", { cols: state.cols, rows: state.rows });
    if (state.exitCode !== null) return t("hook.terminal.exited", { code: state.exitCode });
    return t("hook.terminal.idle");
  }, [state, t]);

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-xl border border-border/70 bg-card p-2.5 text-xs">
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <input
          className="h-7 min-w-[10rem] flex-1 rounded border border-input bg-transparent px-2 font-mono"
          value={program}
          onChange={(e) => setProgram(e.target.value)}
          placeholder={t("hook.terminal.programPh")}
          data-testid="terminal-program"
        />
        <input
          className="h-7 w-40 rounded border border-input bg-transparent px-2 font-mono"
          value={argsText}
          onChange={(e) => setArgsText(e.target.value)}
          placeholder={t("hook.terminal.argsPh")}
          data-testid="terminal-args"
        />
        <Button size="sm" className="h-7 px-2" disabled={busy || !program.trim() || state.running} onClick={() => void start()} data-testid="terminal-start">
          <Play className="h-3.5 w-3.5" />
          {t("hook.terminal.start")}
        </Button>
        <Button size="sm" variant="outline" className="h-7 px-2" disabled={busy || !state.running} onClick={() => void stop()} data-testid="terminal-stop">
          <CircleStop className="h-3.5 w-3.5" />
          {t("hook.terminal.stop")}
        </Button>
        <Button size="sm" variant="ghost" className="h-7 px-2" disabled={busy} onClick={() => void restart()} data-testid="terminal-restart">
          <RotateCcw className="h-3.5 w-3.5" />
          {t("hook.terminal.restart")}
        </Button>
        <span className={cn("text-10px", state.running ? "text-emerald-500" : "text-muted-foreground")} data-testid="terminal-status">
          {statusLine}
        </span>
        {/* 渲染器写出来不是装饰：webgl 起不来时"卡顿"和"空白"是两种完全不同的故障 */}
        {renderer && <span className="text-10px text-muted-foreground">{t("hook.terminal.renderer", { kind: renderer })}</span>}
      </div>
      <p className="shrink-0 text-10px text-muted-foreground">{t("hook.terminal.hint")}</p>
      {error && (
        <p className="shrink-0 break-all text-10px text-destructive" data-testid="terminal-error">
          {error}
        </p>
      )}
      <div
        ref={hostRef}
        className="min-h-0 flex-1 overflow-hidden rounded border border-border/60 bg-black/85 p-1"
        data-testid="terminal-host"
      />
    </section>
  );
}
