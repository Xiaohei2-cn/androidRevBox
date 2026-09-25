import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDownToLine, Ban, Copy, ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import { taskApi } from "@/api/task";
import { parseFridaLine, summarizeEventData, type FridaEvent } from "@/api/hook";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import { stripAnsi } from "@/lib/ansi";
import { AnsiText } from "@/components/ui/AnsiText";
import type { ConsoleEvent, FridaFilter, SessionInfo, TaskState } from "./types";
import {
  DEFAULT_FILTER,
  filterMatch,
  filterStorageKey,
  loadFilter,
  loadWrap,
  saveFilter,
  saveWrap,
} from "./types";

const MAX_EVENTS = 20_000;
/** 高频 send 打爆事件流的兜底：16ms 合并一帧（§10） */
const BATCH_MS = 16;

interface QueuedLine {
  stream: "stdout" | "stderr" | "system";
  line: string;
  ts: number;
}

/**
 * 区域 C · 会话控制台（右 2/3 通高，§5/§5.3-M1）：
 * NDJSON 三类事件分轨渲染、过滤（localStorage 持久化）、虚拟列表钉底跟随、
 * 历史回放（task_logs 重放同一管线）、「原始模式」开关（肌肉记忆兜底，§10）。
 */
export function FridaConsole({
  session,
  onStopped,
}: {
  session: SessionInfo;
  onStopped: (taskId: string) => void;
}) {
  const { t } = useI18n();
  const { gotoTasks } = useAppNav();
  const [events, setEvents] = useState<ConsoleEvent[]>([]);
  const [status, setStatus] = useState<TaskState>("running");
  const [rawMode, setRawMode] = useState(false);
  const [wrap, setWrap] = useState<boolean>(() => loadWrap(true));
  const [filter, setFilter] = useState<FridaFilter>(() => loadFilter(filterStorageKey(session.settings)));
  const nextId = useRef(0);
  const queue = useRef<QueuedLine[]>([]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const historyLoaded = useRef(false);

  // 过滤条件持久化（按设备+包记忆）
  useEffect(() => {
    saveFilter(filterStorageKey(session.settings), filter);
  }, [filter, session.settings]);

  const push = useCallback((lines: QueuedLine[]) => {
    setEvents((prev) => {
      let next = prev;
      for (const q of lines) {
        const evt = q.stream === "stdout" ? parseFridaLine(q.line) : evtForOther(q);
        next = [...next, { id: nextId.current++, ts: q.ts, stream: q.stream, line: q.line, evt }];
      }
      return next.length > MAX_EVENTS ? next.slice(next.length - MAX_EVENTS) : next;
    });
  }, []);

  const enqueue = useCallback(
    (q: QueuedLine) => {
      queue.current.push(q);
      if (!timer.current) {
        timer.current = setTimeout(() => {
          timer.current = null;
          const batch = queue.current;
          queue.current = [];
          if (batch.length) push(batch);
        }, BATCH_MS);
      }
    },
    [push],
  );

  // 实时流：订阅 task://output + task://status（复用既有事件名，按 taskId 过滤，§8-3）
  useEffect(() => {
    let alive = true;
    const unsubs: Array<() => void> = [];
    void taskApi
      .onOutput((p) => {
        if (alive && p.taskId === session.taskId) {
          enqueue({ stream: p.stream, line: p.chunk, ts: Date.now() });
        }
      })
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    void taskApi
      .onStatus((p) => alive && p.taskId === session.taskId && setStatus(p.status))
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    return () => {
      alive = false;
      unsubs.forEach((un) => un());
      if (timer.current) clearTimeout(timer.current);
    };
  }, [session.taskId, enqueue]);

  // 回放：挂载时先灌历史（重开 app 看运行中会话、或已结束会话的历史）
  useEffect(() => {
    if (historyLoaded.current) return;
    historyLoaded.current = true;
    let alive = true;
    void taskApi
      .logs(session.taskId, 5000)
      .then((rows) => {
        if (!alive || rows.length === 0) return;
        setEvents(
          rows.map((r) => ({
            id: nextId.current++,
            ts: r.ts * 1000,
            stream: r.stream,
            line: r.chunk,
            evt: r.stream === "stdout" ? parseFridaLine(r.chunk) : evtForOther({ stream: r.stream, line: r.chunk, ts: r.ts * 1000 }),
          })),
        );
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [session.taskId]);

  const running = status === "running" || status === "pending";
  useEffect(() => {
    if (!running && (status === "success" || status === "failed" || status === "cancelled")) {
      onStopped(session.taskId);
    }
  }, [running, status, session.taskId, onStopped]);

  const visible = useMemo(() => events.filter((e) => filterMatch(e, filter)), [events, filter]);
  const origin = events[0]?.ts ?? null;

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-xl border border-border/70 bg-card shadow-card p-2.5 text-xs">
      <SessionHeader
        session={session}
        status={status}
        total={events.length}
        shown={visible.length}
        rawMode={rawMode}
        onRawMode={setRawMode}
        wrap={wrap}
        onWrap={(v) => {
          setWrap(v);
          saveWrap(v);
        }}
        onCopyTaskId={() => void copyText(session.taskId)}
        onGotoTasks={gotoTasks}
        onStop={running ? () => void taskApi.cancel(session.taskId) : undefined}
      />
      <FilterBar filter={filter} onChange={setFilter} />
      <EventStream
        events={visible}
        rawMode={rawMode}
        wrap={wrap}
        origin={origin}
        running={running}
        emptyText={t(events.length === 0 ? "hook.frida.idle" : "hook.frida.noMatch")}
      />
    </section>
  );
}

/** stderr/system 行：不丢（目标死亡原因、pid 行），降级为 raw 轨道 */
function evtForOther(q: QueuedLine): FridaEvent {
  return { kind: "raw", text: q.line };
}

function copyText(s: string): Promise<void> {
  if (navigator.clipboard) return navigator.clipboard.writeText(s);
  return Promise.reject(new Error("clipboard unavailable"));
}

// ===== 会话头（§5.4） =====

function SessionHeader({
  session,
  status,
  total,
  shown,
  rawMode,
  onRawMode,
  wrap,
  onWrap,
  onCopyTaskId,
  onGotoTasks,
  onStop,
}: {
  session: SessionInfo;
  status: TaskState;
  total: number;
  shown: number;
  rawMode: boolean;
  onRawMode: (v: boolean) => void;
  /** false = 不折行（宽 hexdump 看整行，容器出横向滚动条） */
  wrap: boolean;
  onWrap: (v: boolean) => void;
  onCopyTaskId: () => void;
  onGotoTasks: () => void;
  onStop?: () => void;
}) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const dot =
    status === "running" || status === "pending"
      ? "bg-emerald-500 animate-pulse"
      : status === "cancelled"
        ? "bg-amber-500"
        : status === "failed"
          ? "bg-destructive"
          : "bg-muted-foreground";
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2">
      <span className={cn("h-2 w-2 shrink-0 rounded-full", dot)} />
      <span className="path-selectable min-w-0 flex-1 truncate font-mono font-medium" title={session.name}>
        {session.name}
      </span>
      <span className="shrink-0 text-11px text-muted-foreground">
        {t("hook.frida.rowCount", { shown, total })}
      </span>
      <button
        type="button"
        className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-10px hover:bg-muted/70"
        title={t("hook.frida.copyTaskId")}
        onClick={() => {
          onCopyTaskId();
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        }}
      >
        {copied ? t("common.copied") : session.taskId.slice(0, 8)}
      </button>
      <Button size="sm" variant="ghost" className="h-5 shrink-0 gap-0.5 px-1 text-10px" onClick={onGotoTasks}>
        <ExternalLink className="h-2.5 w-2.5" />
        {t("hook.frida.taskCenter")}
      </Button>
      <label className="flex shrink-0 cursor-pointer items-center gap-1 text-11px text-muted-foreground">
        <input type="checkbox" checked={rawMode} onChange={(e) => onRawMode(e.target.checked)} />
        {t("hook.frida.rawMode")}
      </label>
      {/* 换行开关：hexdump/表格类输出折行后列就散了，读的人需要"横向滚动看整行" */}
      <label className="flex shrink-0 cursor-pointer items-center gap-1 text-11px text-muted-foreground">
        <input
          type="checkbox"
          data-testid="wrap-toggle"
          checked={wrap}
          onChange={(e) => onWrap(e.target.checked)}
        />
        {t("hook.frida.wrapMode")}
      </label>
      {onStop && (
        <Button size="sm" variant="destructive" className="h-6 shrink-0 gap-1 px-2" onClick={onStop}>
          <Ban className="h-3 w-3" />
          {t("hook.frida.stop")}
        </Button>
      )}
    </div>
  );
}

// ===== 过滤栏（§5.3-M1） =====

function FilterBar({ filter, onChange }: { filter: FridaFilter; onChange: (f: FridaFilter) => void }) {
  const { t } = useI18n();
  const chips: { key: keyof FridaFilter; label: string }[] = [
    { key: "send", label: "send" },
    { key: "log", label: "log" },
    { key: "error", label: "error" },
    { key: "raw", label: t("hook.frida.other") },
  ];
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1.5">
      {chips.map((c) => (
        <button
          key={c.key}
          type="button"
          onClick={() => onChange({ ...filter, [c.key]: !filter[c.key] })}
          className={cn(
            "rounded-full border px-2 py-0.5 text-10px transition-colors",
            filter[c.key]
              ? "border-foreground/30 bg-muted text-foreground"
              : "border-border/50 text-muted-foreground opacity-50",
          )}
        >
          {c.label}
        </button>
      ))}
      <input
        className="h-6 min-w-0 flex-1 rounded border border-input bg-transparent px-1.5 font-mono text-11px"
        placeholder={t("hook.frida.filterPh")}
        value={filter.kw}
        onChange={(e) => onChange({ ...filter, kw: e.target.value })}
      />
      <button
        type="button"
        onClick={() => onChange({ ...filter, regex: !filter.regex })}
        className={cn(
          "rounded border px-1.5 py-0.5 font-mono text-10px",
          filter.regex ? "border-sky-500 text-sky-500" : "text-muted-foreground",
        )}
        title=".*"
      >
        .*
      </button>
      {(filter.kw || filter.regex) && (
        <Button size="sm" variant="ghost" className="h-5 px-1 text-10px" onClick={() => onChange(DEFAULT_FILTER)}>
          {t("hook.frida.clear")}
        </Button>
      )}
    </div>
  );
}

// ===== 事件流：虚拟列表 + 钉底跟随（§5.3-M1） =====

// 导出给测试：这一层的价值全在"行高量得准"，不导出来就只能测单行渲染（上一版就是这样，
// 于是"从不测量"这个缺陷一直没被测到）
export function EventStream({
  events,
  rawMode,
  wrap,
  origin,
  running,
  emptyText,
}: {
  events: ConsoleEvent[];
  rawMode: boolean;
  /** 同头部「自动换行」开关 */
  wrap: boolean;
  origin: number | null;
  running: boolean;
  emptyText: string;
}) {
  const { t } = useI18n();
  const parentRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const [newCount, setNewCount] = useState(0);
  const lastLen = useRef(0);

  /**
   * 虚拟列表必须**量过真实高度**，不然就是"挤占"：以前只给 estimateSize=22 却从不测量，
   * 于是任何一条折行的消息（hexdump 一行 438 字符，屏宽下是 3–4 行）实际高 60–90px，
   * 下一行却仍按 22px 的偏移量绝对定位 —— 文本互相压在彼此上面。
   *
   * getItemKey 用事件 id 而不是 index：超过 MAX_EVENTS 会从头裁剪，index 一移位
   * 测量缓存就张冠李戴（表现同样是行高错乱，且只在流跑久了之后出现，最难复现）。
   */
  const rowVirtualizer = useVirtualizer({
    count: events.length,
    getScrollElement: () => parentRef.current,
    getItemKey: (index) => events[index]?.id ?? index,
    estimateSize: () => 22,
    overscan: 12,
  });

  // 只渲染视口内（含 overscan）的行；上下留白把没渲染的部分垫出来
  const items = rowVirtualizer.getVirtualItems();
  const firstItem = items[0];
  const lastItem = items[items.length - 1];

  // 钉底跟随：新行到达且在底部 → 滚到底；用户上滚 → 暂停并计数
  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const added = events.length - lastLen.current;
    lastLen.current = events.length;
    if (added <= 0) return;
    if (pinned) {
      rowVirtualizer.scrollToIndex(Math.max(0, events.length - 1), { align: "end" });
      setNewCount(0);
    } else {
      setNewCount((n) => n + added);
    }
  }, [events.length, pinned, rowVirtualizer]);

  const onScroll = useCallback(() => {
    const el = parentRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    setPinned(atBottom);
    if (atBottom) setNewCount(0);
  }, []);

  const resumeFollow = useCallback(() => {
    setPinned(true);
    setNewCount(0);
    lastLen.current = events.length;
    rowVirtualizer.scrollToIndex(Math.max(0, events.length - 1), { align: "end" });
  }, [events.length, rowVirtualizer]);

  return (
    <div className="relative min-h-0 flex-1" data-testid="frida-stream">
      <div
        ref={parentRef}
        onScroll={onScroll}
        className="h-full overflow-auto rounded-lg border bg-black/80 p-2 font-mono text-11px leading-relaxed dark:bg-black/40"
      >
        {events.length === 0 ? (
          <span className={cn("text-muted-foreground", running && "animate-pulse")}>{emptyText}</span>
        ) : (
          /*
           * 占位块 + 普通流，而不是"每行绝对定位"。
           *
           * 绝对定位那一版把 overlap 做成了可达状态：行高全靠 estimateSize，一旦某条消息
           * 折行成多行（hexdump 一行 438 字符，屏宽下 3–4 行）而测量又没生效，下一条就
           * 直接印在它身上 —— 用户看到的就是"文本互相挤占"。
           * 现在可见行按文档流排：量不准最多只影响滚动条长度与占位，**永远不可能压到别的行**。
           */
          <div
            data-testid="frida-rows"
            style={{
              // 上下留白顶出"没渲染的那部分"高度。故意**不**写死容器高度：
              // 写死了以后，真实行高与估算不一致时内容会溢出去，滚动条长度反而更离谱。
              paddingTop: firstItem?.start ?? 0,
              paddingBottom: lastItem
                ? Math.max(0, rowVirtualizer.getTotalSize() - lastItem.end)
                : 0,
              width: "100%",
            }}
          >
            {items.map((vi) => {
              const e = events[vi.index];
              if (!e) return null;
              return (
                <div
                  key={vi.key}
                  // data-index 是 measureElement 找行的依据；少了它就永远退回估算值
                  data-index={vi.index}
                  data-testid="frida-row"
                  ref={rowVirtualizer.measureElement}
                >
                  <EventRow e={e} raw={rawMode} wrap={wrap} origin={origin} />
                </div>
              );
            })}
          </div>
        )}
      </div>
      {!pinned && newCount > 0 && (
        <Button
          size="sm"
          className="absolute bottom-3 left-1/2 h-7 -translate-x-1/2 gap-1 shadow-lg"
          onClick={resumeFollow}
        >
          <ArrowDownToLine className="h-3 w-3" />
          {t("hook.frida.resumeFollow", { n: newCount })}
        </Button>
      )}
    </div>
  );
}

// ===== 单事件渲染（三色分轨，§5.3-M1） =====

export function EventRow({
  e,
  raw,
  wrap,
  origin,
}: {
  e: ConsoleEvent;
  raw: boolean;
  /** false = 整行不折行，靠横向滚动读 */
  wrap: boolean;
  origin: number | null;
}) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const k = e.evt.kind;
  const color =
    k === "send"
      ? "text-emerald-400"
      : k === "error"
        ? "bg-red-950/60 text-red-300"
        : k === "log"
          ? e.evt.lvl === "error"
            ? "text-red-300"
            : e.evt.lvl === "warn"
              ? "text-amber-300"
              : "text-zinc-300"
          : k === "ready"
            ? "text-sky-400"
            : k === "exit"
              ? "text-amber-400"
              : "text-zinc-500";
  const time = renderTime(e.ts, origin);
  return (
    <div
      className={cn(
        "group flex items-start gap-2 rounded px-1",
        // 折行时优先在空格处断（hexdump 的列不能被从字节中间切开），
        // 只有超长无空格整串才硬断；不折行时整行按原样铺开，交给容器横向滚
        wrap ? "whitespace-pre-wrap [overflow-wrap:anywhere]" : "w-max min-w-full whitespace-pre",
        raw ? "text-zinc-400" : color,
      )}
    >
      <span
        className={cn(
          "shrink-0 select-none bg-black/80 text-10px text-zinc-600 dark:bg-black/40",
          // 不折行时行比视口宽：时间戳与复制键钉在两侧，不然一滚就找不到
          !wrap && "sticky left-0 z-10",
        )}
      >
        {time}
      </span>
      {raw ? (
        <span className="path-selectable min-w-0 flex-1">{e.line}</span>
      ) : (
        <span className="path-selectable min-w-0 flex-1">
          {/* 脚本自己会打 ANSI 颜色（hexdump 的地址绿、零字节黄）：
              当文字渲染就是一行乱码，翻成样式才是它本来的样子 */}
          <AnsiText text={rawlessBody(e.evt, t)} />
          {e.evt.kind === "error" && e.evt.stack && (
            <details className="mt-0.5 text-10px text-red-400/80">
              <summary className="cursor-pointer select-none">{t("hook.frida.stack")}</summary>
              <pre className="whitespace-pre-wrap">
                <AnsiText text={e.evt.stack} />
              </pre>
            </details>
          )}
        </span>
      )}
      <button
        type="button"
        className={cn(
          "shrink-0 rounded bg-black/80 text-zinc-600 opacity-0 hover:text-zinc-300 group-hover:opacity-100 dark:bg-black/40",
          !wrap && "sticky right-0 z-10",
        )}
        title={t("common.copy")}
        onClick={() => {
          // 复制"看见的东西"：普通模式复制去色后的正文，原始模式复制那行 NDJSON 原文。
          // 以前一律复制 wire 行，粘出去是一串 \u001b[0;32m，hexdump 没法直接用。
          void copyText(raw ? e.line : stripAnsi(rawlessBody(e.evt, t))).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1000);
          });
        }}
      >
        <Copy className="h-3 w-3" />
      </button>
      {copied && <span className="shrink-0 text-10px text-zinc-500">{t("common.copied")}</span>}
    </div>
  );
}

/** 非原始模式的单行体（M1；M2 升级成可折叠 JSON 树） */
function rawlessBody(evt: FridaEvent, t: (k: string) => string): string {
  switch (evt.kind) {
    case "ready":
      return `● ${evt.mode} ${evt.pkg}${evt.pid != null ? ` (pid ${evt.pid})` : ""}${evt.frida ? ` · frida ${evt.frida}` : ""}`;
    case "log":
      return evt.msg;
    case "send": {
      const tag = evt.tag ? `${evt.tag} ` : "";
      const seq = evt.seq != null ? `#${evt.seq} ` : "";
      return `▸ ${tag}${seq}${summarizeEventData(evt.data)}`;
    }
    case "error":
      return `✗ ${evt.why}`;
    case "exit":
      return evt.why ? `◇ exit(${evt.code ?? "?"}) ${evt.why}` : `◇ exit(${evt.code ?? "?"})`;
    case "raw":
      return evt.text;
  }
  return t("hook.frida.unknown");
}

function renderTime(ts: number, origin: number | null): string {
  const d = new Date(ts);
  const pad = (x: number, n = 2) => String(x).padStart(n, "0");
  const hms = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
  if (origin != null && ts >= origin) {
    const rel = Math.floor((ts - origin) / 100) / 10;
    return `${hms} +${rel.toFixed(1)}s`;
  }
  return hms;
}
