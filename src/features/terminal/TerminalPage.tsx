/**
 * 真终端页（FT1：分页与会话保活）。
 *
 * 组件只做四件事：把 tab 画出来、把每块屏挂到它自己的宿主 div 上、
 * 把"量得到的尺寸"转成 fit、把用户意图转成会话表的调用。
 * **一个字节都不在这里处理**（§4.1 那条铁律），会话的生死与状态在 `lib/terminalStore`。
 *
 * 三条不可回退的口径：
 * 1. **新开一个不动之前的**：所有 tab 的终端实例都活着，非活动 tab 只是 `hidden`；
 *    验收动作是"第 2 个跑长任务，切到第 1 个再切回来，输出没丢"。
 * 2. **关运行中的会话要确认**：关 tab 会关掉 PTY，内核随即给前台进程组发 SIGHUP，不可逆。
 * 3. **页面卸载收掉全部会话**（R-08）：不留活进程。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, CircleStop, Plus, X } from "lucide-react";

import { envApi } from "@/api/env";
import { ptyApi } from "@/api/pty";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import { fridaReplLaunch } from "@/lib/fridaTerminal";
import { MAX_TERMINAL_SESSIONS, TerminalSessionStore, type TerminalTab } from "@/lib/terminalStore";
import "@xterm/xterm/css/xterm.css";

const DOT_CLASS: Record<TerminalTab["status"], string> = {
  running: "bg-emerald-500",
  starting: "bg-sky-400",
  stopped: "bg-amber-500",
  exited: "bg-muted-foreground/50",
  failed: "bg-destructive",
};

export function TerminalPage() {
  const { t } = useI18n();
  const storeRef = useRef<TerminalSessionStore | null>(null);
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [program, setProgram] = useState("");
  const [argsText, setArgsText] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [presetBusy, setPresetBusy] = useState(false);
  const [pendingClose, setPendingClose] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<number | null>(null);
  const hostsRef = useRef(new Map<string, HTMLDivElement>());
  const refCbsRef = useRef(new Map<string, (el: HTMLDivElement | null) => void>());

  // 会话表**只创建一次**：每次 render 新建会把所有 xterm 实例连缓冲区一起丢掉，
  // 那正是"切个 tab 输出就没了"的成因。
  if (!storeRef.current) {
    storeRef.current = new TerminalSessionStore({
      transport: ptyApi,
      onChange: () => setTabs(storeRef.current?.tabs() ?? []),
    });
  }
  const store = storeRef.current;

  useEffect(() => {
    void ptyApi
      .defaultProgram()
      .then((value) => setProgram((current) => current || value))
      .catch((reason) => setNotice(String((reason as Error)?.message ?? reason)));
  }, []);

  // 离开页面 = 收掉全部会话（R-08）。**只在这一条路径上**批量关会话。
  useEffect(
    () => () => {
      storeRef.current?.disposeAll();
      storeRef.current = null;
    },
    [],
  );

  const active = useMemo(() => tabs.find((tab) => tab.id === activeId) ?? null, [tabs, activeId]);

  /**
   * 每个 tab 一个**稳定身份**的 ref 回调：inline 箭头函数每次 render 都是新函数，
   * React 会先以 null 调一次再以元素调一次，于是每次重渲染都要重新判断挂没挂上。
   * 缓存起来就把这条噪音消掉（StrictMode 的 detach/attach 仍然要能扛，见会话表的 opened）。
   */
  const refFor = useCallback(
    (id: string) => {
      let cb = refCbsRef.current.get(id);
      if (!cb) {
        cb = (el: HTMLDivElement | null) => {
          if (el) hostsRef.current.set(id, el);
          else hostsRef.current.delete(id);
          storeRef.current?.mount(id, el);
        };
        refCbsRef.current.set(id, cb);
      }
      return cb;
    },
    [],
  );

  const select = useCallback((id: string) => {
    setActiveId(id);
    setPendingClose(null);
    // 切回来必须重新量一次：隐藏时量到 0，fit 什么都不改（R-03），
    // "刚变可见"这一刻是唯一能把它对齐的机会（ResizeObserver 也会在这里补一次）
    requestAnimationFrame(() => {
      store.fit(id);
      store.repaint(id);
      store.focus(id);
    });
  }, [store]);

  const createTab = useCallback(async () => {
    setNotice(null);
    const args = argsText.trim() ? argsText.trim().split(/\s+/) : [];
    const created = await store.create({ program, args });
    if (!created) return;
    if (!created.ok) {
      setNotice(
        created.reason === "limit"
          ? t("hook.terminal.limit", { max: created.max })
          : t("hook.terminal.emptyProgram"),
      );
      return;
    }
    setActiveId(created.id);
  }, [argsText, program, store, t]);

  const closeTab = useCallback(
    async (id: string) => {
      const store2 = store;
      if (pendingClose !== id) {
        // 第一次点：只登记"待确认"。运行中的会话在这里就杀进程是不可接受的
        const tab = store2.tabs().find((item) => item.id === id);
        if (!tab || (tab.status !== "running" && tab.status !== "starting")) {
          await finishClose(store2, id, setActiveId, setPendingClose, t);
          return;
        }
        setPendingClose(id);
        return;
      }
      await finishClose(store2, id, setActiveId, setPendingClose, t);
    },
    [pendingClose, store, t],
  );

  /**
   * frida 会话预置（FT3 路 B 的第一步）：把启动命令填成 `<配置的 python> -m frida_tools.repl`。
   *
   * 两件事是刻意的：
   * 1. **按会话类型分开要求**——终端里的 REPL 只认 `frida-tools`（模块在它里面），
   *    不再沿用那句"frida 或 frida-tools 任一即通过"：那种机器上今天会被放行，
   *    然后在终端里报 Python 的 `No module named frida_tools`。
   * 2. **不自动起会话**：官方 REPL 必须有目标（`-f 包名` / `-n 进程名` / PID），
   *    直接起只会让它打一句 usage 就退——把"还差一个目标"说给她，比给她一屏报错有用。
   */
  const applyFridaPreset = useCallback(async () => {
    setNotice(null);
    setHint(null);
    setPresetBusy(true);
    try {
      // **每次都重新探**：她很可能就是刚 `pip install frida-tools` 回来点这个按钮，
      // 缓存会把"已经装好了"说成"没装"，那是我们自己造的假现场
      const python = await envApi.python();
      // 探测本身有剪枝：Python 没就绪时前端根本不该去问 frida（问了也是白问）
      const frida = python.ready ? await envApi.frida() : null;
      const launch = fridaReplLaunch(python, frida);
      if (!launch.ok) {
        setNotice(
          launch.block === "no-python"
            ? t("hook.terminal.fridaNoPython")
            // 走到 no-frida-tools 这一支时 python.path 必然有值（否则前一条就判掉了），
            // 这个 ?? 只是为了让类型闭得上，不是第二条判断
            : t("hook.terminal.fridaNoTools", { python: python.path ?? "python" }),
        );
        return;
      }
      setProgram(launch.program);
      setArgsText(launch.args.join(" "));
      setHint(t("hook.terminal.fridaPresetHint", { version: launch.toolsVersion }));
    } catch (reason) {
      setNotice(String((reason as Error)?.message ?? reason));
    } finally {
      setPresetBusy(false);
    }
  }, [t]);

  const stopActive = useCallback(async () => {
    if (!active) return;
    setNotice(null);
    await store.stop(active.id);
  }, [active, store]);

  const search = useCallback(
    (next: boolean) => {
      if (!active || !query) return;
      const bundle = store.bundle(active.id);
      if (!bundle) return;
      // **只下指令，不自己数命中**：命中数由 addon-search 的结果事件回来（下面的订阅），
      // 自己按 findNext 的布尔值猜一个数，就是把"没有命中"和"命中 1 处"混成一件事
      if (next) bundle.search.findNext(query);
      else bundle.search.findPrevious(query);
    },
    [active, query, store],
  );

  const activeIdForSearch = active?.id ?? null;
  useEffect(() => {
    if (!activeIdForSearch) {
      setHits(null);
      return undefined;
    }
    const bundle = store.bundle(activeIdForSearch);
    if (!bundle) return undefined;
    const disposable = bundle.search.onDidChangeResults((event) => setHits(event.resultCount));
    return () => disposable.dispose();
  }, [activeIdForSearch, store]);

  // 尺寸跟着容器走：每个 tab 各看自己的宿主（隐藏的那个量到 0，不会改动它的尺寸）
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      // 页面整体重新可见（Hook 子标签切走又切回）也走这里：尺寸一从 0 变成有值，
    // 除了量一次尺寸还要补一次重绘，否则隐藏期间到达的字节就一直不进屏
    hostsRef.current.forEach((el, id) => {
      if (store.fit(id)) store.repaint(id);
      else if (el.clientWidth > 0) store.repaint(id);
    });
    });
    hostsRef.current.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [store, tabs.length]);

  const tabCount = tabs.length;
  useEffect(() => {
    if (tabCount === 0) setActiveId(null);
    else if (!tabs.some((tab) => tab.id === activeId)) setActiveId(tabs[tabCount - 1].id);
  }, [tabCount, tabs, activeId]);

  const statusText = useMemo(() => {
    if (!active) return t("hook.terminal.idle");
    switch (active.status) {
      case "running":
        return t("hook.terminal.running", { cols: active.cols, rows: active.rows });
      case "starting":
        return t("hook.terminal.starting");
      case "exited":
        return t("hook.terminal.exited", { code: active.exitCode ?? "—" });
      case "failed":
        return active.error ?? t("hook.terminal.failed");
      default:
        return t("hook.terminal.stopped");
    }
  }, [active, t]);

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
        <Button
          size="sm"
          className="h-7 px-2"
          disabled={!program.trim() || tabCount >= MAX_TERMINAL_SESSIONS}
          onClick={() => void createTab()}
          data-testid="terminal-new"
        >
          <Plus className="h-3.5 w-3.5" />
          {t("hook.terminal.new")}
        </Button>
        {/* 路 B 的入口：把 REPL 的启动命令填进来（检查口径按会话类型分开，见 lib/fridaTerminal） */}
        <Button
          size="sm"
          variant="outline"
          className="h-7 px-2"
          disabled={presetBusy}
          onClick={() => void applyFridaPreset()}
          data-testid="terminal-frida-preset"
        >
          {t("hook.terminal.fridaPreset")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="h-7 px-2"
          disabled={active?.status !== "running"}
          onClick={() => void stopActive()}
          data-testid="terminal-stop"
        >
          <CircleStop className="h-3.5 w-3.5" />
          {t("hook.terminal.stop")}
        </Button>
        <span className={cn("text-10px", active?.status === "running" ? "text-emerald-500" : "text-muted-foreground")} data-testid="terminal-status">
          {statusText}
        </span>
        {tabCount > 0 && (
          <span className="text-10px text-muted-foreground" data-testid="terminal-count">
            {t("hook.terminal.count", { n: tabCount, max: MAX_TERMINAL_SESSIONS })}
          </span>
        )}
        {active && (
          <span className="text-10px text-muted-foreground">{t("hook.terminal.renderer", { kind: active.renderer })}</span>
        )}
      </div>

      {tabs.length > 0 && (
        <div className="flex shrink-0 flex-wrap items-center gap-1" role="tablist" data-testid="terminal-tabs">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={cn(
                "flex items-center gap-1 rounded-t border border-b-0 border-border/60 px-2 py-1",
                tab.id === activeId ? "bg-background font-medium" : "bg-muted/40 text-muted-foreground",
              )}
            >
              <button
                type="button"
                role="tab"
                aria-selected={tab.id === activeId}
                className="flex items-center gap-1"
                onClick={() => select(tab.id)}
                data-testid={`terminal-tab-${tab.id}`}
              >
                <span
                  className={cn("h-1.5 w-1.5 shrink-0 rounded-full", DOT_CLASS[tab.status])}
                  data-testid={`terminal-dot-${tab.id}`}
                />
                <span className="max-w-[10rem] truncate">{tab.title}</span>
              </button>
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                onClick={() => void closeTab(tab.id)}
                aria-label={t("hook.terminal.close")}
                data-testid={`terminal-close-${tab.id}`}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          ))}
        </div>
      )}

      {pendingClose && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 rounded border border-destructive/40 bg-destructive/10 px-2 py-1" data-testid="terminal-confirm">
          <span className="text-destructive">
            {t("hook.terminal.confirmClose", { title: tabs.find((tab) => tab.id === pendingClose)?.title ?? pendingClose })}
          </span>
          <Button size="sm" variant="destructive" className="h-6 px-2" onClick={() => void closeTab(pendingClose)} data-testid="terminal-confirm-yes">
            {t("hook.terminal.confirmYes")}
          </Button>
          <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => setPendingClose(null)} data-testid="terminal-confirm-no">
            {t("hook.terminal.confirmNo")}
          </Button>
        </div>
      )}

      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <input
          className="h-6 w-40 rounded border border-input bg-transparent px-2 font-mono"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setHits(null);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            e.preventDefault();
            search(!e.shiftKey);
          }}
          placeholder={t("hook.terminal.searchPh")}
          disabled={!active}
          data-testid="terminal-search"
        />
        <Button size="sm" variant="ghost" className="h-6 px-1" disabled={!active || !query} onClick={() => search(false)} aria-label={t("hook.terminal.searchPrev")} data-testid="terminal-search-prev">
          <ChevronUp className="h-3.5 w-3.5" />
        </Button>
        <Button size="sm" variant="ghost" className="h-6 px-1" disabled={!active || !query} onClick={() => search(true)} aria-label={t("hook.terminal.searchNext")} data-testid="terminal-search-next">
          <ChevronDown className="h-3.5 w-3.5" />
        </Button>
        {hits !== null && (
          <span className="text-10px text-muted-foreground" data-testid="terminal-hits">
            {hits > 0 ? t("hook.terminal.hits", { n: hits }) : t("hook.terminal.hitsNone")}
          </span>
        )}
      </div>

      {hint && (
        <p className="shrink-0 text-10px text-emerald-500" data-testid="terminal-hint">
          {hint}
        </p>
      )}
      <p className="shrink-0 text-10px text-muted-foreground">{t("hook.terminal.hint")}</p>
      {notice && (
        <p className="shrink-0 break-all text-10px text-destructive" data-testid="terminal-error">
          {notice}
        </p>
      )}

      {/* 每块屏都有自己的宿主：非活动 tab 只是 hidden，实例与缓冲区一直活着（分页那条验收的根） */}
      <div className="relative min-h-0 flex-1">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            ref={refFor(tab.id)}
            hidden={tab.id !== activeId}
            className="absolute inset-0 overflow-hidden rounded border border-border/60 bg-black/85 p-1"
            data-testid={tab.id === activeId ? "terminal-host" : `terminal-host-${tab.id}`}
          />
        ))}
      </div>
    </section>
  );
}

/** 真的关一个 tab：关会话 + 把活动 tab 挪到它旁边的兄弟（关最后一个时挪到新的最后一个）。 */
async function finishClose(
  store: TerminalSessionStore,
  id: string,
  setActiveId: (id: string | null) => void,
  setPendingClose: (id: string | null) => void,
  t: (key: string, params?: Record<string, string | number>) => string,
): Promise<void> {
  const before = store.tabs();
  const at = before.findIndex((tab) => tab.id === id);
  const result = await store.close(id, { force: true });
  setPendingClose(null);
  if (!result.closed) {
    // 走到这里说明会话表自己不同意关——把原因显示出来，不假装关掉了
    return;
  }
  const neighbour = before[at + 1]?.id ?? before[at - 1]?.id ?? null;
  setActiveId(neighbour);
  void t;
}
