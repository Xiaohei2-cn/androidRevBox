import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  Copy,
  Regex,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { deviceApi, type ProcReadResult } from "@/api/device";
import type { ProcPath } from "@/api/env";
import { useI18n } from "@/i18n";
import { PathText } from "@/components/ui/PathText";
import { cn } from "@/lib/utils";
import { compileSearch, countMatches, isInvalidQuery, splitLine } from "./procSearch";

/**
 * `/proc/<pid>` 关键路径这一栏（设备信息页）。
 *
 * 分成"读到的摘要"和"要点才读"两类，是这个组件存在的理由：
 * `maps` 属于**别的用户的进程**，Agent 以 shell 身份读它一定 `EACCES`（Android 14 实测，
 * 只有 root 读得到）。以前每次刷新都替它跑一次 `wc -l`，既白跑，又在界面上留一句
 * 红色的"不可读"——看着像工具坏了，其实是权限边界，而且用户想看的是内容不是那个字。
 * 现在这一项标成 `readOnDemand`，界面上只给一个箭头：不点不发请求，点了才跳详情去读。
 */
/**
 * 一次读多少行。原来这里是 400，而 `maps` 一万行是常态——界面上只看得到第一段大块
 * 分配，用户在终端 `cat` 的结果永远对不上，看起来就像"软件显示不全"。
 * 现在与 Agent 的默认值对齐成 20000（Agent 上限 100000 行 / 32 MiB，先到先截）。
 */
const READ_MAX_LINES = 20_000;

type DetailTarget = { serial: string; pid: number; file: "maps" | "cmdline" | "status" };

export function ProcPaths({
  serial,
  pid,
  entries,
}: {
  serial: string;
  /** 前台应用的 pid（字符串形态来自设备信息 DTO） */
  pid?: string | null;
  entries: ProcPath[];
}) {
  const { t } = useI18n();
  const [detail, setDetail] = useState<DetailTarget | null>(null);
  const pidNumber = Number(pid ?? "");

  if (detail && Number.isFinite(pidNumber) && pidNumber > 0) {
    return (
      <ProcFileDetail
        target={detail}
        onBack={() => setDetail(null)}
        onReload={() => setDetail({ ...detail })}
      />
    );
  }

  return (
    <ul className="space-y-1" data-testid={`fg-proc-paths-${serial}`}>
      {entries.length === 0 && (
        <li className="text-muted-foreground">{t("dashboard.foreground.noProc")}</li>
      )}
      {entries.map((entry) => {
        const file = entry.name as "maps" | "cmdline" | "status";
        const openable = file === "maps" || file === "cmdline" || file === "status";
        return (
          <li key={entry.path} className="flex items-baseline gap-2 font-mono">
            <PathText value={entry.path} className="shrink-0 text-foreground" />
            {entry.readOnDemand ? (
              // 「没读」不等于「读不到」：这里不给红字，只给入口
              <span className="min-w-0 flex-1 break-all text-muted-foreground">
                {t("dashboard.foreground.readOnDemand")}
              </span>
            ) : (
              <span
                className={cn(
                  "min-w-0 flex-1 break-all",
                  entry.readable ? "text-muted-foreground" : "text-amber-500",
                )}
                title={entry.summary ?? undefined}
              >
                {entry.readable ? entry.summary || t("common.emptyValue") : t("dashboard.foreground.unreadable")}
              </span>
            )}
            {openable && (
              <Button
                size="sm"
                variant="ghost"
                type="button"
                className="h-6 shrink-0 gap-1 px-1.5 text-[11px]"
                data-testid={`fg-proc-open-${serial}-${file}`}
                aria-label={`${t("dashboard.foreground.viewDetail")} ${entry.path}`}
                disabled={!Number.isFinite(pidNumber) || pidNumber <= 0}
                onClick={() => setDetail({ serial, pid: pidNumber, file })}
              >
                {/* 入口必须看得见文字：只放一个图标就是上一轮"悬停才显示 = 用户以为
                    没这个功能"的同一个坑，只是换了个藏法 */}
                <ChevronRight className="h-3.5 w-3.5" />
                {t("dashboard.foreground.viewDetail")}
              </Button>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function ProcFileDetail({
  target,
  onBack,
  onReload,
}: {
  target: DetailTarget;
  onBack: () => void;
  onReload: () => void;
}) {
  const { t } = useI18n();
  const [nonce, setNonce] = useState(0);
  const [searchOpen, setSearchOpen] = useState(false);
  const query = useQuery<ProcReadResult>({
    // 只在点开之后才发：queryKey 里有 file，切换文件不会复用旧内容
    queryKey: ["device", "proc-read", target.serial, target.pid, target.file, nonce],
    queryFn: () =>
      deviceApi.procRead(target.serial, target.pid, target.file, READ_MAX_LINES),
    enabled: true,
    retry: false,
    staleTime: 2_000,
  });

  const copy = () => {
    if (query.data) void navigator.clipboard?.writeText(query.data.text);
  };

  // 换文件/换 pid 时搜索框收起，免得带着上一份文件的查询条件看新内容
  useEffect(() => {
    setSearchOpen(false);
  }, [target.file, target.pid, target.serial]);

  return (
    <div
      className="rounded-xl border border-border/70 bg-card shadow-card p-2"
      data-testid={`fg-proc-detail-${target.serial}-${target.file}`}
    >
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 gap-1 px-1.5 text-[11px]"
          data-testid="fg-proc-detail-back"
          onClick={onBack}
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {t("dashboard.foreground.detailBack")}
        </Button>
        <PathText
          value={query.data?.path ?? `/proc/${target.pid}/${target.file}`}
          className="min-w-0 flex-1 font-mono text-[11px]"
        />
        <Button
          size="sm"
          variant={searchOpen ? "secondary" : "ghost"}
          type="button"
          className="h-6 gap-1 px-1.5 text-[11px]"
          data-testid="fg-proc-detail-search-toggle"
          aria-label={t("dashboard.foreground.search")}
          aria-pressed={searchOpen}
          disabled={!query.data}
          onClick={() => setSearchOpen((open) => !open)}
        >
          <Search className="h-3.5 w-3.5" />
          {/* 这份文件动辄几千行，入口只放图标就是"用户以为没这个功能" */}
          {t("dashboard.foreground.search")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 px-1.5"
          aria-label={t("common.copy")}
          disabled={!query.data}
          onClick={copy}
        >
          <Copy className="h-3.5 w-3.5" />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 px-1.5"
          aria-label={t("common.refresh")}
          disabled={query.isFetching}
          onClick={() => {
            setNonce((n) => n + 1);
            onReload();
          }}
        >
          <RefreshCw className={cn("h-3.5 w-3.5", query.isFetching && "animate-spin")} />
        </Button>
      </div>
      <p className="mt-1 text-[11px] text-muted-foreground" data-testid="fg-proc-detail-meta">
        {query.isFetching && !query.data
          ? t("dashboard.foreground.detailLoading")
          : query.data
            ? detailMeta(t, query.data)
            : query.isError
              ? `${t("dashboard.foreground.detailFailed")}：${String(
                  (query.error as Error)?.message ?? query.error,
                )}`
              : t("common.none")}
      </p>
      {query.data && searchOpen ? (
        <ProcSearchProvider>
          <ProcSearchBar />
          {/* 重新读取后内容可能变了，查询条件不动，高亮跟着新内容重算 */}
          <ProcFileBody text={query.data.text} searchOpen key={`${target.file}:${nonce}`} />
        </ProcSearchProvider>
      ) : query.data ? (
        <ProcFileBody text={query.data.text} searchOpen={false} />
      ) : null}
    </div>
  );
}

interface ProcSearchState {
  query: string;
  caseSensitive: boolean;
  regex: boolean;
  /** 只看命中行（过滤），而不是高亮后自己眯着眼找 */
  onlyMatches: boolean;
  /** 当前定位到第几处命中（0 基，用于回车/Shift+回车在命中间跳） */
  current: number;
  /** 总命中数：正文算出来写回，状态栏和跳转都要用 */
  count: number;
}

/**
 * 搜索状态提到详情层：输入框与高亮正文是同一份状态，各自持有一份就会"框里改了、
 * 正文没跟"。用 context 而非 props 下钻，是因为正文那边还需要 current 做当前命中定位。
 */
const ProcSearchContext = createContext<{
  state: ProcSearchState;
  setState: (patch: Partial<ProcSearchState>) => void;
} | null>(null);

/**
 * 查询条件与"当前第几处命中"放同一份状态里，输入框和正文共用；命中总数由正文
 * 算完写回（正文才知道真实行数）。查询条件一变就把 current 归零，否则改了一个字后
 * 序号还停在上一轮的偏移上，看起来像"跳到了莫名其妙的位置"。
 */
function ProcSearchProvider({ children }: { children: React.ReactNode }) {
  const [state, setRaw] = useState<ProcSearchState>({
    query: "",
    caseSensitive: false,
    regex: false,
    onlyMatches: false,
    current: 0,
    count: 0,
  });
  // 只改查询条件时把定位归零；改 current/count 本身（回车翻命中、正文回填）不能归零，
  // 否则回车刚翻到下一处就被 effect 拉回第 0 处
  const setState = (patch: Partial<ProcSearchState>) =>
    setRaw((prev) => {
      const next = { ...prev, ...patch };
      const conditionChanged =
        next.query !== prev.query ||
        next.caseSensitive !== prev.caseSensitive ||
        next.regex !== prev.regex ||
        next.onlyMatches !== prev.onlyMatches;
      return conditionChanged ? { ...next, current: 0 } : next;
    });
  return (
    <ProcSearchContext.Provider value={{ state, setState }}>
      {children}
    </ProcSearchContext.Provider>
  );
}

function useProcSearch() {
  const ctx = useContext(ProcSearchContext);
  // 开发者错误：搜索栏与高亮正文必须挂在同一个 Provider 下，缺了就是接线漏了
  if (!ctx) throw new Error("ProcSearchProvider is missing");
  return ctx;
}

function ProcSearchBar() {
  const { t } = useI18n();
  const { state, setState } = useProcSearch();
  const inputRef = useRef<HTMLInputElement>(null);
  const invalid = isInvalidQuery(state.query, state);
  const hasQuery = state.query.length > 0 && !invalid;

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div className="mt-1.5 space-y-1" data-testid="fg-proc-search">
      <div className="flex items-center gap-1.5">
        <div className="relative min-w-0 flex-1">
          <Input
            ref={inputRef}
            value={state.query}
            data-testid="fg-proc-search-input"
            aria-label={t("dashboard.foreground.search")}
            aria-invalid={invalid}
            placeholder={
              state.regex
                ? t("dashboard.foreground.searchRegexPlaceholder")
                : t("dashboard.foreground.searchPlaceholder")
            }
            className={cn(
              "h-7 pr-6 font-mono text-[11px]",
              invalid && "border-amber-500/60 focus-visible:ring-amber-500/40",
            )}
            onChange={(e) => setState({ query: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                setState({
                  current: (state.current + (e.shiftKey ? -1 : 1) + state.count) % Math.max(state.count, 1),
                });
              } else if (e.key === "Escape") {
                e.preventDefault();
                setState({ query: "" });
              }
            }}
          />
          {state.query ? (
            <button
              type="button"
              className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
              aria-label={t("dashboard.foreground.searchClear")}
              data-testid="fg-proc-search-clear"
              onClick={() => setState({ query: "" })}
            >
              <X className="h-3 w-3" />
            </button>
          ) : null}
        </div>
        <ToggleChip
          active={state.caseSensitive}
          testId="fg-proc-search-case"
          label={t("dashboard.foreground.searchCaseSensitive")}
          title={t("dashboard.foreground.searchCaseSensitiveHint")}
          onClick={() => setState({ caseSensitive: !state.caseSensitive })}
        >
          <CaseSensitive className="h-3.5 w-3.5" />
        </ToggleChip>
        <ToggleChip
          active={state.regex}
          testId="fg-proc-search-regex"
          label={t("dashboard.foreground.searchRegex")}
          title={t("dashboard.foreground.searchRegexHint")}
          onClick={() => setState({ regex: !state.regex })}
        >
          <Regex className="h-3.5 w-3.5" />
        </ToggleChip>
        <ToggleChip
          active={state.onlyMatches}
          testId="fg-proc-search-only"
          label={t("dashboard.foreground.searchOnlyMatches")}
          title={t("dashboard.foreground.searchOnlyMatchesHint")}
          onClick={() => setState({ onlyMatches: !state.onlyMatches })}
        >
          {/* 只是"过滤到命中行"，不是跳转，所以不用箭头 */}
          <ChevronDown className="h-3.5 w-3.5" />
        </ToggleChip>
      </div>
      <p
        className={cn(
          "text-[10.5px]",
          invalid ? "text-amber-500" : "text-muted-foreground",
        )}
        data-testid="fg-proc-search-status"
      >
        {invalid
          ? t("dashboard.foreground.searchInvalidRegex")
          : hasQuery && state.count === 0
            ? t("dashboard.foreground.searchNoMatch")
            : hasQuery
              ? t("dashboard.foreground.searchMatches", { count: state.count })
              : ""}
      </p>
    </div>
  );
}

function ToggleChip({
  active,
  label,
  title,
  testId,
  onClick,
  children,
}: {
  active: boolean;
  label: string;
  title: string;
  testId: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={label}
      aria-pressed={active}
      data-testid={testId}
      onClick={onClick}
      className={cn(
        "flex h-7 shrink-0 items-center gap-0.5 rounded-md border px-1.5 text-[10.5px] transition-colors",
        active
          ? "border-primary/50 bg-primary/10 text-primary"
          : "border-input text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
      {/* 图标本身有歧义（尤其 Aa / .*），旁边补一个字，省得靠悬停才知道是什么 */}
      <span>{label}</span>
    </button>
  );
}

function ProcFileBody({
  text,
  searchOpen,
}: {
  text: string;
  searchOpen: boolean;
}) {
  const { t } = useI18n();
  const ctx = useContext(ProcSearchContext);
  const state = ctx?.state;
  const activeRef = useRef<HTMLElement | null>(null);

  // 依赖拆到具体字段，而不是整个 state：state 里还有 count/current，那是本组件写回去的，
  // 拿它当依赖会在"写回 → 重算 matcher → 再写回"之间多空转一轮
  const query = state?.query ?? "";
  const caseSensitive = state?.caseSensitive ?? false;
  const regex = state?.regex ?? false;
  const current = state?.current ?? 0;

  const matcher = useMemo(
    () => (searchOpen ? compileSearch(query, { caseSensitive, regex }) : null),
    [searchOpen, query, caseSensitive, regex],
  );

  const lines = useMemo(() => (text ? text.split("\n") : []), [text]);

  // 每行命中数 + 全局序号起址：current 是全局命中序号，这里先算好每行的偏移
  const rows = useMemo(() => {
    let offset = 0;
    const built = lines.map((line) => {
      const count = matcher ? countMatches(line, matcher) : 0;
      const row = { line, count, offset };
      offset += count;
      return row;
    });
    return { rows: built, total: offset };
  }, [lines, matcher]);

  // 命中总数只有正文知道，算完写回给状态栏和回车跳转用；顺带把越界的 current 收回
  const setState = ctx?.setState;
  const total = matcher ? rows.total : 0;
  const count = state?.count ?? 0;
  useEffect(() => {
    if (!setState) return;
    const clamped = total === 0 ? 0 : Math.min(current, total - 1);
    if (count !== total || current !== clamped) setState({ count: total, current: clamped });
  }, [total, count, current, setState]);

  // 当前命中滚进视野；换查询/切命中时不必把手动滚动位置拽回来
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [current, query, caseSensitive, regex]);

  if (!text) {
    return (
      <pre
        className="pane-scroll mt-1 max-h-64 overflow-auto rounded bg-muted/40 p-1.5 font-mono text-[10.5px] leading-snug"
        data-testid="fg-proc-detail-body"
      >
        {t("common.emptyValue")}
      </pre>
    );
  }

  // 只在"有查询且开了过滤"时裁行；清空查询要立刻回到全文，不能让用户以为文件只剩几行
  const filtering = Boolean(searchOpen && state?.onlyMatches && matcher);
  let hitCursor = 0;
  let activeSeen = false;

  return (
    <pre
      className="pane-scroll mt-1 max-h-64 overflow-auto rounded bg-muted/40 p-1.5 font-mono text-[10.5px] leading-snug"
      data-testid="fg-proc-detail-body"
    >
      {rows.rows.map((row, i) => {
        if (filtering && row.count === 0) return null;
        const segments = matcher ? splitLine(row.line, matcher) : [{ text: row.line, hit: false }];
        return (
          <span key={i} className="block" data-testid="fg-proc-detail-line">
            {segments.map((segment, j) => {
              if (!segment.hit) return <span key={j}>{segment.text}</span>;
              const isActive = hitCursor === current;
              hitCursor += 1;
              // 当前命中留个 ref 给滚动定位；同一帧里只认第一个，其余走普通高亮
              const ref =
                isActive && !activeSeen
                  ? (el: HTMLElement | null) => {
                      activeRef.current = el;
                      activeSeen = true;
                    }
                  : undefined;
              return (
                <mark
                  key={j}
                  ref={ref}
                  data-active={isActive ? "true" : undefined}
                  className={cn(
                    "rounded-[2px] px-0 text-inherit",
                    isActive
                      ? "bg-amber-400/70 text-black ring-1 ring-amber-500"
                      : "bg-amber-400/35 text-inherit",
                  )}
                >
                  {segment.text}
                </mark>
              );
            })}
            {/* 行尾要给一个真实字符，否则空行整行塌掉，行号对不上 */}
            {row.line.length === 0 ? "\u00a0" : null}
          </span>
        );
      })}
    </pre>
  );
}

function detailMeta(t: (key: string, values?: Record<string, string | number>) => string, data: ProcReadResult) {
  const via =
    data.read_via === "root"
      ? t("dashboard.foreground.detailViaRoot")
      : t("dashboard.foreground.detailViaShell");
  // 三件事都要看得见：读到多少、有没有被截断、这次是谁的权限
  return data.truncated
    ? t("dashboard.foreground.detailLinesTruncated", {
        total: data.total_lines,
        shown: data.returned_lines,
        via,
      })
    : t("dashboard.foreground.detailLines", {
        total: data.total_lines,
        shown: data.returned_lines,
        via,
      });
}
