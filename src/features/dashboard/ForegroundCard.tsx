import { useQuery } from "@tanstack/react-query";
import { Smartphone } from "lucide-react";
import { deviceApi } from "@/api/device";
import { envApi, type ForegroundApp } from "@/api/env";
import { EnvCard } from "./EnvCards";
import { cn } from "@/lib/utils";

/**
 * 安卓前台应用卡（P7）——仪表盘唯一允许独占一整行的卡，且必须排在最底。
 * 数据链：env_foreground（后端 adb 不可用 → 0 次 shell 调用直接剪枝）。
 * 轮询前置：adb 就绪才轮询；无在线设备降频为慢轮询兜底（拔线即停密集刷新）。
 */

const FAST_POLL_MS = 5_000;
const SLOW_POLL_MS = 30_000;

export function ForegroundCard() {
  const { data: adbEnv } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });

  const { data: devices } = useQuery({
    queryKey: ["devices"],
    queryFn: () => deviceApi.list(),
    enabled: !!adbEnv?.installed,
    refetchInterval: SLOW_POLL_MS,
  });

  const deviceOnline = !!devices?.some((d) => d.state === "device");

  const { data, isFetching, refetch, dataUpdatedAt } = useQuery({
    queryKey: ["env", "foreground", adbEnv?.installed ?? false, deviceOnline],
    queryFn: envApi.foreground,
    // 剪枝：adb 未就绪不发起前台探测；有设备 5s 轮询，无设备 30s 兜底
    enabled: !!adbEnv?.installed,
    refetchInterval: deviceOnline ? FAST_POLL_MS : SLOW_POLL_MS,
    staleTime: 4_000,
    retry: false,
  });

  return (
    <div className="col-span-2" data-testid="foreground-card-wrapper">
      <EnvCard
        testid="foreground-card"
        icon={Smartphone}
        title="安卓前台应用"
        refresh={() => void refetch()}
        refreshing={isFetching}
        disabled={!adbEnv?.installed}
        status={statusOf(data, !!adbEnv?.installed)}
      >
        <ForegroundBody app={data} lastUpdated={dataUpdatedAt} />
      </EnvCard>
    </div>
  );
}

function statusOf(app: ForegroundApp | undefined, adbInstalled: boolean) {
  if (!adbInstalled) return { tone: "muted" as const, label: "adb 不可用" };
  if (app === undefined) return { tone: "muted" as const, label: "检测中…" };
  switch (app.state) {
    case "ready":
      return { tone: "ok" as const, label: "检测中（实时）" };
    case "no_device":
      return { tone: "muted" as const, label: "无在线设备" };
    case "no_foreground":
      return { tone: "muted" as const, label: "无前台应用" };
    case "error":
      return { tone: "warn" as const, label: "检测失败" };
    default:
      return { tone: "muted" as const, label: "暂停" };
  }
}

function ForegroundBody({
  app,
  lastUpdated,
}: {
  app: ForegroundApp | undefined;
  lastUpdated: number;
}) {
  if (!app) return null;
  if (app.state !== "ready") {
    return <CardHint>{app.error ?? app.hint}</CardHint>;
  }
  return (
    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 text-xs md:grid-cols-2">
      <Field label="包名" value={app.package} testid="fg-package" />
      <Field label="Activity" value={app.activity} testid="fg-activity" />
      <Field label="PID" value={app.pid} testid="fg-pid" mono />
      <Field label="native lib 目录" value={app.nativeLibDir} testid="fg-libdir" mono />
      <div className="md:col-span-2">
        <p className="mb-1 text-muted-foreground">/proc 关键路径</p>
        <ul className="space-y-1" data-testid="fg-proc-paths">
          {app.procPaths.length === 0 && (
            <li className="text-muted-foreground">进程未运行（pid 不可得）</li>
          )}
          {app.procPaths.map((p) => (
            <li key={p.path} className="flex items-baseline gap-2 font-mono">
              <span className="shrink-0 text-foreground">{p.path}</span>
              <span
                className={cn(
                  "truncate",
                  p.readable ? "text-muted-foreground" : "text-amber-500",
                )}
                title={p.summary ?? undefined}
              >
                {p.readable
                  ? p.summary || "（空）"
                  : "不可读（可能需要 root）"}
              </span>
            </li>
          ))}
        </ul>
      </div>
      <p className="md:col-span-2 text-right text-[10px] text-muted-foreground">
        设备 {app.serial} · 更新于 {new Date(lastUpdated).toLocaleTimeString()}
      </p>
    </div>
  );
}

function Field({
  label,
  value,
  mono,
  testid,
}: {
  label: string;
  value?: string | null;
  mono?: boolean;
  testid: string;
}) {
  return (
    <p className="flex items-baseline gap-2">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span
        data-testid={testid}
        className={cn("truncate", mono ? "font-mono" : "font-medium")}
        title={value ?? undefined}
      >
        {value || "—"}
      </span>
    </p>
  );
}

function CardHint({ children }: { children?: React.ReactNode }) {
  return <p className="text-xs leading-relaxed text-muted-foreground">{children ?? "—"}</p>;
}
