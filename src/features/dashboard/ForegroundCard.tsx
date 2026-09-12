import { useQuery } from "@tanstack/react-query";
import { Smartphone } from "lucide-react";
import { deviceApi } from "@/api/device";
import { envApi, type ForegroundApp } from "@/api/env";
import { useActiveTab } from "@/app/nav";
import { useI18n } from "@/i18n";
import { PathText } from "@/components/ui/PathText";
import { EnvCard } from "./EnvCards";
import { cn } from "@/lib/utils";

/**
 * 安卓前台应用卡（P7）——仪表盘唯一允许独占一整行的卡，且必须排在最底。
 * 数据链：env_foreground（后端 adb 不可用 → 0 次 shell 调用直接剪枝）。
 * 轮询前置：adb 就绪才轮询；无在线设备降频为慢轮询兜底（拔线即停密集刷新）；
 * keep-mounted 后仅仪表盘激活时轮询（§10 减少后台开销）。
 */

const FAST_POLL_MS = 5_000;
const SLOW_POLL_MS = 30_000;

export function ForegroundCard() {
  const { t } = useI18n();
  const active = useActiveTab("dashboard");
  const { data: adbEnv } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });

  const { data: devices } = useQuery({
    queryKey: ["devices"],
    queryFn: () => deviceApi.list(),
    enabled: !!adbEnv?.installed && active,
    refetchInterval: active ? SLOW_POLL_MS : false,
  });

  const deviceOnline = !!devices?.some((d) => d.state === "device");

  const { data, isFetching, refetch, dataUpdatedAt } = useQuery({
    queryKey: ["env", "foreground", adbEnv?.installed ?? false, deviceOnline],
    queryFn: envApi.foreground,
    // 剪枝：adb 未就绪不发起前台探测；有设备 5s 轮询，无设备 30s 兜底（均仅激活时）
    enabled: !!adbEnv?.installed && active,
    refetchInterval: active ? (deviceOnline ? FAST_POLL_MS : SLOW_POLL_MS) : false,
    staleTime: 4_000,
    retry: false,
  });

  return (
    <div className="col-span-2" data-testid="foreground-card-wrapper">
      <EnvCard
        testid="foreground-card"
        icon={<Smartphone className="h-4 w-4" />}
        title={t("dashboard.foreground.title")}
        refresh={() => void refetch()}
        refreshing={isFetching}
        disabled={!adbEnv?.installed}
        status={statusOf(data, !!adbEnv?.installed, t)}
      >
        <ForegroundBody app={data} lastUpdated={dataUpdatedAt} t={t} />
      </EnvCard>
    </div>
  );
}

function statusOf(
  app: ForegroundApp | undefined,
  adbInstalled: boolean,
  t: (k: string, v?: Record<string, string | number>) => string,
) {
  if (!adbInstalled) return { tone: "muted" as const, label: t("dashboard.foreground.adbUnavailable") };
  if (app === undefined) return { tone: "muted" as const, label: t("common.loading") };
  switch (app.state) {
    case "ready":
      return { tone: "ok" as const, label: t("dashboard.foreground.live") };
    case "no_device":
      return { tone: "muted" as const, label: t("dashboard.foreground.noDevice") };
    case "no_foreground":
      return { tone: "muted" as const, label: t("dashboard.foreground.noForeground") };
    case "error":
      return { tone: "warn" as const, label: t("common.detectFailed") };
    default:
      return { tone: "muted" as const, label: t("dashboard.foreground.paused") };
  }
}

function ForegroundBody({
  app,
  lastUpdated,
  t,
}: {
  app: ForegroundApp | undefined;
  lastUpdated: number;
  t: (k: string, v?: Record<string, string | number>) => string;
}) {
  if (!app) return null;
  if (app.state !== "ready") {
    return <CardHint>{app.error ?? app.hint}</CardHint>;
  }
  return (
    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 text-xs md:grid-cols-2">
      <Field label={t("dashboard.foreground.package")} value={app.package} testid="fg-package" />
      <Field label={t("dashboard.foreground.activity")} value={app.activity} testid="fg-activity" />
      <Field label={t("dashboard.foreground.pid")} value={app.pid} testid="fg-pid" mono />
      <Field label={t("dashboard.foreground.nativeLib")} value={app.nativeLibDir} testid="fg-libdir" mono />
      <div className="md:col-span-2">
        <p className="mb-1 text-muted-foreground">{t("dashboard.foreground.procPaths")}</p>
        <ul className="space-y-1" data-testid="fg-proc-paths">
          {app.procPaths.length === 0 && (
            <li className="text-muted-foreground">{t("dashboard.foreground.noProc")}</li>
          )}
          {app.procPaths.map((p) => (
            <li key={p.path} className="flex items-baseline gap-2 font-mono">
              <PathText value={p.path} className="shrink-0 text-foreground" />
              <span
                className={cn(
                  "min-w-0 flex-1 truncate",
                  p.readable ? "text-muted-foreground" : "text-amber-500",
                )}
                title={p.summary ?? undefined}
              >
                {p.readable
                  ? p.summary || t("common.emptyValue")
                  : t("dashboard.foreground.unreadable")}
              </span>
            </li>
          ))}
        </ul>
      </div>
      <p className="md:col-span-2 text-right text-xs text-muted-foreground">
        {t("common.device")} <PathText value={app.serial} className="font-mono" /> ·{" "}
        {t("common.updatedAt")} {new Date(lastUpdated).toLocaleTimeString()}
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
    <p className="flex min-w-0 items-baseline gap-2">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <PathText
        value={value}
        testid={testid}
        className={cn("min-w-0 flex-1", mono ? "font-mono" : "font-medium")}
      />
    </p>
  );
}

function CardHint({ children }: { children?: React.ReactNode }) {
  return <p className="text-xs leading-relaxed text-muted-foreground">{children ?? "—"}</p>;
}
