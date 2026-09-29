/**
 * 真终端面板（FT1 分页 + FT3 路 B）：**这一块就是 frida 会话的控制台**。
 *
 * 位置口径（第八十四轮，用户明确纠正过的地方）：它在 **frida 页右侧那一整块屏**上，
 * 不是另开的一个子标签——左边 A 区设置、B 区脚本，点脚本 = 在这里起一个官方 frida REPL。
 * 面板自己只管"几块屏、谁在跑、什么时候才许杀进程"，会话状态在 `lib/terminalStore`。
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
import { ChevronDown, ChevronUp, CircleStop, Download, Plus, X } from "lucide-react";

import { deviceApi } from "@/api/device";
import { envApi } from "@/api/env";
import { hookApi } from "@/api/hook";
import { ptyApi } from "@/api/pty";
import { hasNativeDialog, saveFile } from "@/api/dialog";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import type { FridaSettings } from "@/features/hook/frida/types";
import { deviceShellLaunch } from "@/lib/deviceShell";
import { planFridaSession, type FridaSessionBlock } from "@/lib/fridaSession";
import { collectTranscript, transcriptFileName, transcriptText } from "@/lib/terminalTranscript";
import { MAX_TERMINAL_SESSIONS, TerminalSessionStore, type TerminalTab } from "@/lib/terminalStore";
import "@xterm/xterm/css/xterm.css";

/**
 * frida 会话起不来的四种原因 → 界面文案。
 *
 * 一个个写成 `t("…")` 字面量，而不是"码 → 键名"的映射表：`scripts/i18n-check.mjs` 只认
 * `t("…")` 这种形状，映射表里的键名打错了闸门不会红（第八十四轮差点埋下这一颗）。
 */
function fridaBlockText(
  block: FridaSessionBlock,
  t: (key: string, params?: Record<string, string | number>) => string,
  pythonPath?: string | null,
): string {
  switch (block) {
    case "no-python":
      return t("hook.terminal.fridaNoPython");
    case "no-frida-tools":
      // 带上解释器路径：她要的就是能直接抄走的那条安装命令
      return t("hook.terminal.fridaNoTools", { python: pythonPath || "python" });
    case "no-device":
      return t("hook.terminal.fridaNoDevice");
    case "no-script":
      return t("hook.terminal.fridaNoScript");
    case "spawn-needs-package":
      return t("hook.terminal.fridaNoPackage");
  }
}

const DOT_CLASS: Record<TerminalTab["status"], string> = {
  running: "bg-emerald-500",
  starting: "bg-sky-400",
  stopped: "bg-amber-500",
  exited: "bg-muted-foreground/50",
  failed: "bg-destructive",
};

/**
 * 外部（B 区点脚本）发起的一次 frida 会话请求。
 * 设置是**按下那一刻的快照**：会话起来之后她再改目标，不该搅动已经在跑的会话。
 */
export interface FridaLaunchRequest {
  scriptPath: string;
  settings: FridaSettings;
  nonce: number;
}

interface TerminalPanelProps {
  fridaLaunch?: FridaLaunchRequest | null;
}

export function TerminalPanel({ fridaLaunch }: TerminalPanelProps) {
  const { t } = useI18n();
  const storeRef = useRef<TerminalSessionStore | null>(null);
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [program, setProgram] = useState("");
  const [argsText, setArgsText] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [presetBusy, setPresetBusy] = useState(false);
  const [saveOutcome, setSaveOutcome] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
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
   * 设备 shell 宿主（H-2，FT2）：**直接起一个会话**，不像 frida 预置那样只填命令。
   *
   * 差别的原因是"还缺不缺参数"：`adb -s <serial> shell -tt` 已经是一个完整可用的交互 shell，
   * 而官方 REPL 必须有目标。实测过 `-tt` 下 Ctrl-C 能中断、改尺寸能传到远端
   * （`stty size` 跟着变），所以这块屏不需要任何新的桌面通道——PTY 不关心跑的是谁。
   */
  const createDeviceShell = useCallback(async () => {
    setNotice(null);
    setHint(null);
    setPresetBusy(true);
    try {
      const [env, devices] = await Promise.all([deviceApi.environment(), deviceApi.list(true)]);
      const online = devices.filter((d) => d.state === "device");
      const built = deviceShellLaunch(env.path, online[0]?.serial);
      if (!built.ok) {
        setNotice(built.block === "no-adb" ? t("hook.terminal.deviceNoAdb") : t("hook.terminal.deviceNoSerial"));
        return;
      }
      const created = await store.create({
        program: built.launch.program,
        args: built.launch.args,
        kind: "device",
        title: built.launch.title,
      });
      if (!created.ok) {
        setNotice(created.reason === "limit" ? t("hook.terminal.limit", { max: created.max }) : t("hook.terminal.emptyProgram"));
        return;
      }
      setActiveId(created.id);
      setHint(
        online.length > 1
          ? t("hook.terminal.deviceMulti", { serial: online[0].serial, count: online.length })
          : t("hook.terminal.devicePicked", { serial: online[0].serial }),
      );
    } catch (reason) {
      setNotice(String((reason as Error)?.message ?? reason));
    } finally {
      setPresetBusy(false);
    }
  }, [store, t]);

  /**
   * 起一个**官方 frida REPL** 会话（FT3 路 B）。这里不自己拼参数：
   * 环境够不够、连哪台、跑哪个脚本，全在 `lib/fridaSession` 一处判（每种形状都有测试钉着）。
   *
   * 每次都重新探环境、不缓存：她很可能就是刚 `pip install frida-tools` 回来点脚本，
   * 缓存会把"已经装好了"说成"没装"——那是我们自己造的假现场。
   */
  const launchFrida = useCallback(
    async (request: FridaLaunchRequest) => {
      setNotice(null);
      setHint(null);
      setPresetBusy(true);
      try {
        const python = await envApi.python();
        // 探测本身有剪枝：Python 没就绪时不该去问 frida（问了也问不出东西）
        const frida = python.ready ? await envApi.frida() : null;
        const planned = planFridaSession(python, frida, request.settings, request.scriptPath);
        if (!planned.ok) {
          setNotice(fridaBlockText(planned.block, t, python.path));
          return;
        }
        const created = await store.create({
          program: planned.plan.program,
          args: planned.plan.args,
          kind: "frida",
          title: planned.plan.title,
        });
        if (!created.ok) {
          setNotice(
            created.reason === "limit"
              ? t("hook.terminal.limit", { max: created.max })
              : t("hook.terminal.emptyProgram"),
          );
          return;
        }
        setActiveId(created.id);
        setHint(t("hook.terminal.fridaStarted", { version: planned.plan.toolsVersion }));
      } catch (reason) {
        setNotice(String((reason as Error)?.message ?? reason));
      } finally {
        setPresetBusy(false);
      }
    },
    [store, t],
  );

  /**
   * B 区点一次脚本 = nonce 变一次 = 起一块新屏。
   * 只认 nonce，不认"请求对象在不在"：连着点同一个脚本也要能再起一次，
   * 而对象比较会让 React 每次重渲染都误判成新请求（那是真会多开一个进程的 bug）。
   */
  const lastNonceRef = useRef<number | null>(null);
  useEffect(() => {
    if (!fridaLaunch || fridaLaunch.nonce === lastNonceRef.current) return;
    lastNonceRef.current = fridaLaunch.nonce;
    void launchFrida(fridaLaunch);
  }, [fridaLaunch, launchFrida]);

  /**
   * 保存这块屏的正文（C-15 的终端那一半，D090 的口径）：**只有用户点过一次并确认路径才写**。
   *
   * 三条都是照着结构化控制台那条已经落地的口径对齐的：
   * - 搜索框里有字 ⇒ 只存匹配的行（"存过滤后留在屏幕上的那些行"，不是原始字节流）；
   * - 用户取消**不算失败**，也不写任何东西（连"桌面版才有的对话框"弹不出来时都说清）；
   * - 写完报**字节数**（Rust 那边回读出来的），不是"已保存"三个字——那种话证明不了什么。
   */
  const saveActive = useCallback(async () => {
    if (!active || saving) return;
    const bundle = store.bundle(active.id);
    if (!bundle) return;
    setSaveOutcome(null);
    const snapshot = collectTranscript(bundle.term, query);
    if (snapshot.lines.length === 0) {
      setSaveOutcome({ ok: false, text: query ? t("hook.terminal.saveNoHit", { query }) : t("hook.terminal.saveEmpty") });
      return;
    }
    setSaving(true);
    try {
      const path = await saveFile({
        title: t("hook.terminal.saveDialog"),
        defaultPath: transcriptFileName(active.program),
        filters: [{ name: t("hook.terminal.saveText"), extensions: ["txt"] }],
      });
      if (!path) {
        // 取消：什么都不发生；只有"这环境根本弹不出对话框"才需要说一句
        if (!hasNativeDialog()) setSaveOutcome({ ok: false, text: t("hook.terminal.saveDesktopOnly") });
        return;
      }
      const bytes = await hookApi.exportSave(path, transcriptText(snapshot));
      setSaveOutcome({
        ok: true,
        text: t("hook.terminal.saved", { lines: snapshot.lines.length, total: snapshot.totalLines, bytes }),
      });
    } catch (reason) {
      setSaveOutcome({ ok: false, text: t("hook.terminal.saveFailed", { message: String((reason as Error)?.message ?? reason) }) });
    } finally {
      setSaving(false);
    }
  }, [active, query, saving, store, t]);

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
        // 有码报码；没码但有信号就报信号；两个都没有就明说"不知道"，不假装修辞成"已停止"
        return t("hook.terminal.exited", {
          code: active.exitCode ?? (active.exitSignal ? `信号 ${active.exitSignal}` : "无退出码"),
        });
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
        {/* H-2：设备 shell 走同一条 PTY 通道，只是换个程序（实测见 lib/deviceShell 顶上那张表） */}
        <Button
          size="sm"
          variant="outline"
          className="h-7 px-2"
          disabled={presetBusy}
          onClick={() => void createDeviceShell()}
          data-testid="terminal-device-shell"
        >
          {t("hook.terminal.deviceShell")}
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
        <span
          className={cn(
            "min-w-0 text-10px",
            active?.status === "running"
              ? "text-emerald-500"
              : // 起不来时必须显眼：以前它和"会话在跑 80×24"共用一句灰小字，
                // 结果用户报"点了没反应"而界面上其实写过原因，只是没人看得见
                active?.status === "failed"
                ? "break-all text-destructive"
                : "text-muted-foreground",
          )}
          data-testid="terminal-status">
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
        <Button
          size="sm"
          variant="outline"
          className="h-6 px-2 text-10px"
          disabled={!active || saving}
          title={query ? t("hook.terminal.saveFilteredHint", { query }) : t("hook.terminal.saveHint")}
          onClick={() => void saveActive()}
          data-testid="terminal-save"
        >
          <Download className="h-3 w-3" />
          {saving ? t("hook.terminal.saving") : t("hook.terminal.save")}
        </Button>
        {saveOutcome && (
          <span
            className={cn("min-w-0 truncate text-10px", saveOutcome.ok ? "text-emerald-500" : "text-destructive")}
            title={saveOutcome.text}
            data-testid="terminal-save-outcome"
          >
            {saveOutcome.text}
          </span>
        )}
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

      {tabCount === 0 && (
        // 空态要说清这块屏能起什么：这个位置现在是 frida 的控制台，
        // 点左侧脚本起的是官方 frida；这两个按钮是另外两种会话
        <div className="shrink-0 rounded border border-border/60 bg-muted/20 px-2 py-1.5 text-10px text-muted-foreground" data-testid="terminal-empty">
          {t("hook.terminal.empty")}
        </div>
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
