import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Bug,
  CheckCircle2,
  CircleSlash,
  Hexagon,
  Plug,
  RefreshCw,
  TerminalSquare,
  XCircle,
  type LucideIcon,
} from "lucide-react";
import { envApi, type McpEnv } from "@/api/env";
import { cn } from "@/lib/utils";

/**
 * 仪表盘环境/工具卡片（P7）：Python / Node / Frida / IDA MCP / jadx MCP。
 * 布局契约：左右两列网格，任何卡不得独占一行；状态三色——
 * 绿=就绪、琥珀=未配置/未检测到（常态，不算错误）、红=探测异常。
 */

export function PythonCard() {
  const { data, isFetching, refetch } = useEnvQuery(["env", "python"], envApi.python);
  return (
    <EnvCard
      testid="env-python"
      icon={TerminalIcon}
      title="Python 环境"
      refresh={() => void refetch()}
      refreshing={isFetching}
      status={
        data === undefined
          ? { tone: "muted", label: "检测中…" }
          : data.ready
            ? { tone: "ok", label: `Python ${data.version}` }
            : data.configured
              ? { tone: "warn", label: "不可用" }
              : { tone: "warn", label: "未配置" }
      }
    >
      {data === undefined ? null : data.ready ? (
        <CardLine mono>{data.path}</CardLine>
      ) : (
        <CardLine>{data.hint}</CardLine>
      )}
    </EnvCard>
  );
}

export function NodeCard() {
  const { data, isFetching, refetch } = useEnvQuery(["env", "node"], envApi.node);
  return (
    <EnvCard
      testid="env-node"
      icon={HexagonIcon}
      title="Node 环境"
      refresh={() => void refetch()}
      refreshing={isFetching}
      status={
        data === undefined
          ? { tone: "muted", label: "检测中…" }
          : data.ready
            ? { tone: "ok", label: `Node ${data.version}` }
            : { tone: "warn", label: "未检测到" }
      }
    >
      {data === undefined ? null : data.ready ? (
        <>
          <CardLine mono>{data.path}</CardLine>
          {data.npmGlobalRoot && (
            <CardLine mono>
              npm 全局包：{data.npmGlobalRoot}
            </CardLine>
          )}
        </>
      ) : (
        <CardLine>{data.hint}</CardLine>
      )}
    </EnvCard>
  );
}

/** Frida 卡依赖 Python 就绪（§10 剪枝：未就绪不发起检测查询） */
export function FridaCard({ pythonReady }: { pythonReady: boolean | undefined }) {
  const enabled = pythonReady === true;
  const { data, isFetching, refetch } = useEnvQuery(["env", "frida"], envApi.frida, enabled);
  return (
    <EnvCard
      testid="env-frida"
      icon={BugIcon}
      title="Frida"
      refresh={() => void refetch()}
      refreshing={isFetching}
      disabled={!enabled}
      status={
        !enabled
          ? { tone: "muted", label: "待 Python 就绪" }
          : data === undefined
            ? { tone: "muted", label: "检测中…" }
            : data.installed
              ? { tone: "ok", label: "已安装" }
              : { tone: "warn", label: "未安装" }
      }
    >
      {!enabled ? (
        <CardLine>先在 设置 → 工具环境 配置可用的 Python 解释器</CardLine>
      ) : data === undefined ? null : data.installed ? (
        <CardLine mono>
          frida {data.fridaVersion ?? "—"}
          {data.fridaToolsVersion ? ` · frida-tools ${data.fridaToolsVersion}` : ""}
        </CardLine>
      ) : (
        <CardLine>{data.hint}</CardLine>
      )}
    </EnvCard>
  );
}

/** IDA / jadx MCP 状态卡（同形组件，端口可配置；连不上是常态不打红） */
export function McpCard({
  testid,
  title,
  queryFn,
}: {
  testid: string;
  title: string;
  queryFn: () => Promise<McpEnv>;
}) {
  const { data, isFetching, refetch } = useEnvQuery(["env", testid], queryFn);
  return (
    <EnvCard
      testid={testid}
      icon={PlugIcon}
      title={title}
      refresh={() => void refetch()}
      refreshing={isFetching}
      status={
        data === undefined
          ? { tone: "muted", label: "检测中…" }
          : data.reachable
            ? { tone: "ok", label: `在线 · ${data.port}` }
            : { tone: "warn", label: "未检测到" }
      }
    >
      {data === undefined ? null : data.reachable ? (
        <CardLine mono>127.0.0.1:{data.port}</CardLine>
      ) : (
        <CardLine>{data.hint}</CardLine>
      )}
    </EnvCard>
  );
}

// ===== 通用骨架 =====

export type StatusTone = "ok" | "warn" | "muted";

export function EnvCard({
  testid,
  icon: Icon,
  title,
  status,
  refresh,
  refreshing,
  disabled,
  children,
}: {
  testid: string;
  icon: LucideIcon;
  title: string;
  status: { tone: StatusTone; label: string };
  refresh: () => void;
  refreshing: boolean;
  disabled?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div data-testid={testid} className="rounded-xl border bg-card p-4" aria-disabled={disabled}>
      <div className="flex items-center gap-2">
        <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />
        <span className="truncate text-sm font-semibold">{title}</span>
        <button
          type="button"
          aria-label={`刷新 ${title}`}
          disabled={refreshing || disabled}
          onClick={refresh}
          className="ml-auto rounded p-1 text-muted-foreground hover:bg-accent disabled:opacity-30"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", refreshing && "animate-spin")} />
        </button>
      </div>
      <div className="mt-2 flex items-center gap-1.5 text-sm font-medium">
        <StatusMark tone={status.tone} />
        <span data-testid={`${testid}-status`}>{status.label}</span>
      </div>
      <div className="mt-1.5 space-y-1">{children}</div>
    </div>
  );
}

function StatusMark({ tone }: { tone: StatusTone }) {
  if (tone === "ok") return <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500" />;
  if (tone === "warn") return <XCircle className="h-3.5 w-3.5 text-amber-500" />;
  return <CircleSlash className="h-3.5 w-3.5 text-muted-foreground" />;
}

function CardLine({ mono, children }: { mono?: boolean; children?: React.ReactNode }) {
  return (
    <p
      className={cn(
        "break-all text-xs leading-relaxed text-muted-foreground",
        mono && "font-mono",
      )}
    >
      {children}
    </p>
  );
}

function useEnvQuery<T>(
  key: readonly unknown[],
  queryFn: () => Promise<T>,
  enabled = true,
) {
  return useQuery({
    queryKey: key,
    queryFn,
    enabled,
    staleTime: 30_000,
    retry: false,
  });
}

// 轻量图标别名（卡片标题用，避免与业务命名冲突）
const TerminalIcon = TerminalSquare;
const HexagonIcon = Hexagon;
const BugIcon = Bug;
const PlugIcon = Plug;

/** 供设置页改动后整体失效环境查询 */
export function useInvalidateEnvQueries() {
  const qc = useQueryClient();
  return () => void qc.invalidateQueries({ queryKey: ["env"] });
}
