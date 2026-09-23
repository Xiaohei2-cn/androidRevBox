import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, RefreshCw, Smartphone, PackageOpen, Rocket, CircleStop, Download, Upload, ShieldCheck, Cpu, Save, FolderPlus, Pencil, KeyRound, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PathBar } from "@/features/files/PathBar";
import { usePathHistory } from "@/features/files/usePathHistory";
import { SubTabs } from "@/components/nav/SubTabs";
import { TaskLaunchPanel } from "@/components/task/TaskLaunchPanel";
import { TaskSessionView } from "@/components/task/TaskSessionView";
import {
  deviceApi,
  type DeviceEntry,
  type OperationStep,
  type PackageUninstallResult,
  type PackageWriteResult,
  type WriteOutcome,
} from "@/api/device";
import { zygiskApi, type ZygiskAppItem, type ZygiskScope } from "@/api/zygisk";
import { agentApi, type AgentSessionState } from "@/api/agent";
import { envApi } from "@/api/env";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { PathText } from "@/components/ui/PathText";
import { BrandIcon } from "@/components/ui/BrandIcon";
import { cn } from "@/lib/utils";
import { pickDirectory, pickFile } from "@/api/dialog";

/**
 * 设备页（P3）：设备列表/信息/Shell/文件/应用/Logcat 六个分 tab。
 * 长操作（shell/logcat/install/uninstall/push/pull）一律走 TaskService 任务，
 * 内联展示实时输出；设备热插拔由后端 watch 线程事件驱动刷新。
 */
/** 包写操作的展示模型：三态（executed / replayed / noOp）与复核结论必须分开显示。 */
interface PackageWriteView {
  kind: string;
  outcome: WriteOutcome;
  verified: boolean;
  summary: string;
  steps: OperationStep[];
  detail?: string;
  failed?: boolean;
}

export function DevicesPage() {
  const [selected, setSelected] = useState<string | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const { pendingDevicesTab } = useAppNav();
  // 子 tab 受控：导航信号到达时切到对应 tab（list/info）
  const [subTab, setSubTab] = useState("list");
  useEffect(() => {
    if (pendingDevicesTab) setSubTab(pendingDevicesTab);
  }, [pendingDevicesTab]);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });

  const {
    data: devices = [],
    isError,
    refetch,
  } = useQuery({
    queryKey: ["devices", refreshTick],
    queryFn: () => deviceApi.list(),
    enabled: !!env?.installed,
    refetchInterval: 10_000,
  });

  const onDeviceEvent = useCallback(() => setRefreshTick((n) => n + 1), []);
  useEffect(() => {
    let alive = true;
    const unsubs: Array<() => void> = [];
    deviceApi
      .onChanged(() => alive && onDeviceEvent())
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    return () => {
      alive = false;
      unsubs.forEach((un) => un());
    };
  }, [onDeviceEvent]);

  // 选中设备保活：掉线自动切到第一个可用
  const activeSerial = useMemo(() => {
    if (devices.some((d) => d.serial === selected)) return selected;
    return devices.find((d) => d.state === "device")?.serial ?? devices[0]?.serial ?? null;
  }, [devices, selected]);

  if (!env) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
        检测 adb 环境…
      </div>
    );
  }
  if (!env.installed) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
        <p className="text-sm font-medium">未检测到 adb</p>
        <p className="max-w-md text-xs leading-relaxed text-muted-foreground">{env.hint}</p>
        <Button size="sm" variant="outline" onClick={() => void refetch()}>
          <RefreshCw className="h-3.5 w-3.5" />
          重试
        </Button>
      </div>
    );
  }

  return (
    <SubTabs
      value={subTab}
      onValueChange={setSubTab}
      tabs={[
        {
          id: "list",
          label: "设备列表",
          content: (
            <DeviceListView
              devices={devices}
              selected={activeSerial}
              onSelect={setSelected}
              isError={isError}
            />
          ),
        },
        {
          id: "info",
          label: "设备信息",
          content: <DeviceCardsView devices={devices} activeSerial={activeSerial} />,
        },
        {
          id: "shell",
          label: "Shell",
          content: (
            <TaskLaunchPanel
              label="命令"
              placeholder="如：input text hello（回车执行，Ctrl+C 无法用，请点停止取消）"
              note="ADB 原始会话：命令原样交给 adb shell，不经 Agent，也没有 typed 错误码。设备侧能力请改用各功能页。"
              disabled={!activeSerial}
              onRun={async (input) => {
                if (!activeSerial || !input.trim()) return null;
                return deviceApi.shell(activeSerial, input);
              }}
            />
          ),
        },
        {
          id: "files",
          label: "文件",
          content: <FilesView serial={activeSerial} />,
        },
        {
          id: "apps",
          label: "应用",
          content: <AppsView serial={activeSerial} />,
        },
        {
          id: "logcat",
          label: "Logcat",
          content: (
            <TaskLaunchPanel
              label="过滤（正则，留空全量）"
              placeholder="如 crash，回车开流；停止按钮取消任务"
              note="ADB 原始会话：adb logcat 流由 Desktop 直读（传输/调试工具），保留取消与日志回放。"
              disabled={!activeSerial}
              onRun={async (filter) => {
                if (!activeSerial) return null;
                return deviceApi.logcat(activeSerial, filter.trim() || undefined);
              }}
            />
          ),
        },
      ]}
    />
  );
}

function DeviceListView({
  devices,
  selected,
  onSelect,
  isError,
}: {
  devices: DeviceEntry[];
  selected: string | null;
  onSelect: (s: string) => void;
  isError: boolean;
}) {
  const { t } = useI18n();
  const { gotoDeviceInfo } = useAppNav();
  if (isError) {
    return <p className="text-xs text-destructive">adb devices 调用失败，检查设备授权或 adb 环境。</p>;
  }
  if (devices.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Smartphone className="h-8 w-8 opacity-40" />
        <p className="text-xs">未连接设备。插入设备并允许 USB 调试后自动出现。</p>
      </div>
    );
  }
  return (
    <ul className="space-y-2">
      {devices.map((d) => (
        <li key={d.serial}>
          <button
            type="button"
            onClick={() => onSelect(d.serial)}
            className={cn(
              "flex w-full items-center gap-3 rounded-lg border p-3 text-left text-xs transition-colors hover:bg-accent",
              d.serial === selected && "border-primary bg-primary/10",
            )}
          >
            <span
              className={cn(
                "h-2 w-2 shrink-0 rounded-full",
                d.state === "device"
                  ? "bg-emerald-500"
                  : d.state === "unauthorized"
                    ? "bg-amber-500"
                    : "bg-muted-foreground",
              )}
            />
            <div className="min-w-0 flex-1">
              <p className="break-all font-medium">{d.model || "未知型号"}</p>
              <p className="break-all font-mono text-muted-foreground">{d.serial}</p>
            </div>
            <span className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
              {d.transport}
            </span>
            <span
              className={cn(
                "w-20 text-right text-xs",
                d.state === "device" ? "text-emerald-500" : "text-amber-500",
              )}
            >
              {STATE_LABEL[d.state] ?? d.state}
            </span>
            <Button
              size="sm"
              variant="outline"
              className="h-7 shrink-0 px-2"
              disabled={d.state !== "device"}
              title={t("devices.gotoInfo")}
              data-testid={`goto-info-${d.serial}`}
              onClick={(e) => {
                e.stopPropagation();
                onSelect(d.serial);
                gotoDeviceInfo(d.serial);
              }}
            >
              {t("devices.gotoInfo")}
            </Button>
          </button>
        </li>
      ))}
    </ul>
  );
}

const STATE_LABEL: Record<string, string> = {
  device: "已就绪",
  unauthorized: "待授权",
  offline: "离线",
};

/**
 * 设备信息 tab（P8 重构）：每台在线设备一张横向拉满的卡。
 * 上半：设备属性（getprop）；分割线；下半：该设备的前台应用（原仪表盘信息）。
 * 「去设备信息」导航到达时自动滚动定位到对应卡。
 */
function DeviceCardsView({
  devices,
  activeSerial,
}: {
  devices: DeviceEntry[];
  activeSerial: string | null;
}) {
  const { pendingDeviceSerial, clearPendingDevice } = useAppNav();
  const online = devices.filter((d) => d.state === "device");
  const containerRef = useRef<HTMLDivElement>(null);

  // 导航定位：pending serial 的卡滚动到可视区 + 高亮闪烁
  useEffect(() => {
    if (!pendingDeviceSerial) return;
    const el = containerRef.current?.querySelector(
      `[data-device-card="${pendingDeviceSerial}"]`,
    );
    el?.scrollIntoView({ behavior: "smooth", block: "start" });
    el?.classList.add("config-flash");
    const timer = window.setTimeout(() => {
      el?.classList.remove("config-flash");
      clearPendingDevice();
    }, 1300);
    return () => window.clearTimeout(timer);
  }, [pendingDeviceSerial, clearPendingDevice, online.length]);

  if (online.length === 0) {
    return <Empty text="无在线设备：连接设备后此页显示设备卡片。" />;
  }
  return (
    <div ref={containerRef} className="flex h-full flex-col gap-4 overflow-auto pb-2">
      {online.map((d) => (
        <DeviceFullCard key={d.serial} serial={d.serial} transport={d.transport} />
      ))}
      {activeSerial === null && null}
    </div>
  );
}

/** 单设备整卡：上=设备属性，分割线，下=前台应用 */
function DeviceFullCard({ serial, transport }: { serial: string; transport: string }) {
  const { t } = useI18n();
  const { data, isLoading, error } = useQuery({
    queryKey: ["device", "info", serial],
    queryFn: () => deviceApi.info(serial),
    enabled: !!serial,
    staleTime: 60_000,
  });
  // Root 横幅：su -c id → uid=0 探测（独立查询；失败/无 root 视觉分级，醒目）
  const { data: rooted, isError: rootProbeFailed } = useQuery({
    queryKey: ["device", "root", serial],
    queryFn: () => deviceApi.rootCheck(serial),
    enabled: !!serial,
    staleTime: 30_000,
    retry: false,
  });

  const rows: [string, string | undefined][] = [
    [t("devices.info.model"), data?.model],
    [t("devices.info.manufacturer"), data?.manufacturer],
    [t("devices.info.androidVersion"), data?.androidVersion],
    [t("devices.info.sdk"), data?.sdkInt],
    [t("devices.info.serial"), serial],
    [t("devices.info.transport"), transport],
    [
      t("devices.info.ip"),
      data?.ip ?? (data === undefined ? undefined : t("devices.info.ipUnavailable")),
    ],
  ];

  const rootBanner = (() => {
    if (rootProbeFailed || rooted === undefined) {
      return {
        cls: "bg-amber-500/15 text-amber-600 dark:text-amber-400 border-amber-500/40",
        label: t("devices.root.probing"),
      };
    }
    return rooted
      ? {
          cls: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 border-emerald-500/50",
          label: t("devices.root.has"),
        }
      : {
          cls: "bg-red-500/15 text-red-600 dark:text-red-400 border-red-500/50",
          label: t("devices.root.none"),
        };
  })();

  return (
    <div
      data-device-card={serial}
      className="w-full rounded-xl border bg-card p-4"
    >
      {/* 上：设备信息 */}
      <div className="flex items-center gap-2">
        <BrandIcon name="android" />
        <span className="text-sm font-semibold">{data?.model || serial}</span>
        <span
          className={cn(
            "ml-auto rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground",
          )}
        >
          {transport}
        </span>
      </div>
      <div
        data-testid={`root-banner-${serial}`}
        className={cn(
          "mt-3 flex items-center gap-2 rounded-lg border px-3 py-1.5 text-sm font-bold tracking-wide",
          rootBanner.cls,
        )}
      >
        <ShieldCheck className="h-4 w-4 shrink-0" />
        ROOT · {rootBanner.label}
      </div>
      <dl className="mt-3 grid grid-cols-[110px_1fr] gap-y-2 text-xs sm:grid-cols-[110px_1fr_110px_1fr]">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="min-w-0 break-all font-mono">
              <PathText value={v || undefined} />
            </dd>
          </div>
        ))}
      </dl>
      {isLoading && <p className="mt-2 text-xs text-muted-foreground">读取属性中…</p>}
      {error && (
        <p className="mt-2 text-xs text-destructive">
          {t("devices.info.loadFailed", { error: String((error as Error)?.message ?? error) })}
        </p>
      )}

      <AgentSessionSection serial={serial} />

      {/* 分割线 */}
      <hr className="my-4 border-border" />

      {/* 下：前台应用（原仪表盘信息，按设备查询） */}
      <ForegroundAppSection serial={serial} />
    </div>
  );
}

/* 导出以便 M1 页面回归渲染（内部仍按原样使用）。 */
/* 导出供 M1 页面回归渲染使用；内部用法不变。 */
export function AgentSessionSection({ serial }: { serial: string }) {
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const queryKey = ["agent", "diagnostics", serial] as const;
  const { data } = useQuery({
    queryKey,
    queryFn: () => agentApi.diagnostics(serial),
    refetchInterval: 10_000,
    retry: false,
  });
  const refresh = () => void queryClient.invalidateQueries({ queryKey });
  const install = useMutation({ mutationFn: () => agentApi.install(serial), onSettled: refresh });
  const restart = useMutation({ mutationFn: () => agentApi.restart(serial), onSettled: refresh });
  const status = data?.status;
  const latestRoute = data?.routes?.[0];
  const state = status?.state ?? "disconnected";
  const available = status?.capabilities.filter((capability) => capability.available).length ?? 0;
  const mutationError = install.error ?? restart.error;
  const busy = install.isPending || restart.isPending;

  return (
    <section className="mt-4 border-t pt-3" data-testid={`agent-status-${serial}`}>
      <div className="flex flex-wrap items-center gap-2">
        <Cpu className="h-4 w-4 text-muted-foreground" />
        <span className="text-xs font-semibold">Android Agent</span>
        <span className={cn("rounded px-1.5 py-0.5 text-xs font-medium", AGENT_STATE_CLASS[state])}>
          {t(`devices.agent.state.${state}`)}
        </span>
        <div className="ml-auto flex items-center gap-1.5">
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2 text-xs"
            disabled={busy}
            onClick={() => (state === "disconnected" ? install.mutate() : restart.mutate())}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", busy && "animate-spin")} />
            {state === "disconnected" ? t("devices.agent.install") : t("devices.agent.restart")}
          </Button>
        </div>
      </div>
      {status && state !== "disconnected" && (
        <dl className="mt-2 grid grid-cols-[90px_1fr] gap-y-1 text-xs sm:grid-cols-[90px_1fr_90px_1fr]">
          <div className="contents">
            <dt className="text-muted-foreground">{t("devices.agent.version")}</dt>
            <dd className="font-mono">{status.agentVersion ?? "-"}</dd>
          </div>
          <div className="contents">
            <dt className="text-muted-foreground">{t("devices.agent.protocol")}</dt>
            <dd className="font-mono">{status.protocolVersion ?? "-"}</dd>
          </div>
          <div className="contents">
            <dt className="text-muted-foreground">{t("devices.agent.providers")}</dt>
            <dd>{status.providers.length}</dd>
          </div>
          <div className="contents">
            <dt className="text-muted-foreground">{t("devices.agent.legacyFallbacks")}</dt>
            {/* 计数为 0 才是"可以删回退腿"的证据；有数字就说明这台机仍在靠 ADB 兜底 */}
            <dd
              className={
                (data?.legacyFallbacks?.length ?? 0) > 0
                  ? "font-mono text-amber-500"
                  : "font-mono text-muted-foreground"
              }
              data-testid="agent-legacy-fallbacks"
            >
              {(data?.legacyFallbacks ?? []).length === 0
                ? t("devices.agent.legacyFallbackNone")
                : (data?.legacyFallbacks ?? [])
                    .map((f) => `${f.method} × ${f.count}（${f.reason}）`)
                    .join(" · ")}
            </dd>
          </div>
          <div className="contents">
            <dt className="text-muted-foreground">{t("devices.agent.capabilities")}</dt>
            <dd>{available}/{status.capabilities.length}</dd>
          </div>
        </dl>
      )}
      {latestRoute && (
        <div className="mt-2 flex min-w-0 items-center gap-2 text-xs">
          <span className="shrink-0 text-muted-foreground">{t("devices.agent.route")}</span>
          <span className="truncate font-mono" title={latestRoute.method}>{latestRoute.method}</span>
          <span className="shrink-0 text-muted-foreground">
            {t(`devices.agent.backend.${latestRoute.backend}`)}
            {latestRoute.fallbackReason ? ` (${latestRoute.fallbackReason})` : ""}
          </span>
        </div>
      )}
      {(mutationError || status?.lastError || data?.healthError) && (
        <p className="mt-2 break-all text-xs text-destructive">
          {String((mutationError as Error | undefined)?.message ?? status?.lastError ?? data?.healthError)}
        </p>
      )}
    </section>
  );
}

const AGENT_STATE_CLASS: Record<AgentSessionState, string> = {
  disconnected: "bg-muted text-muted-foreground",
  adb_online: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
  installing: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  starting: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  handshaking: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
  ready: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
  degraded: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  incompatible: "bg-red-500/15 text-red-600 dark:text-red-400",
};

/** 设备卡下半部：该设备当前前台应用 + /proc 关键路径（复用仪表盘数据链） */
function ForegroundAppSection({ serial }: { serial: string }) {
  const { t } = useI18n();
  const { data, isFetching, refetch } = useQuery({
    queryKey: ["env", "foreground", serial],
    queryFn: () => envApi.foreground(serial),
    staleTime: 4_000,
    refetchInterval: 5_000,
    retry: false,
  });

  const app = data;

  return (
    <div>
      <div className="flex items-center gap-2">
        <Smartphone className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold">{t("devices.foreground.title")}</span>
        <button
          type="button"
          aria-label={t("common.refresh")}
          disabled={isFetching}
          onClick={() => void refetch()}
          className="ml-auto rounded p-1 text-muted-foreground hover:bg-accent disabled:opacity-30"
        >
          <RefreshCw className={cn("h-3 w-3", isFetching && "animate-spin")} />
        </button>
      </div>
      {app === undefined ? (
        <p className="mt-1.5 text-xs text-muted-foreground">{t("common.loading")}</p>
      ) : app.state !== "ready" ? (
        <p className="mt-1.5 text-xs text-muted-foreground">
          {app.error ?? app.hint ?? t("dashboard.foreground.noForeground")}
        </p>
      ) : (
        <div className="mt-1.5 grid grid-cols-1 gap-x-6 gap-y-1.5 text-xs md:grid-cols-2">
          <p className="flex min-w-0 items-baseline gap-2">
            <span className="shrink-0 text-muted-foreground">
              {t("dashboard.foreground.package")}
            </span>
            <KindDot kind={app.packageKind} />
            <PathText
              value={app.package}
              testid={`fg-package-${serial}`}
              className="min-w-0 flex-1 font-medium"
            />
          </p>
          <Field label={t("dashboard.foreground.activity")} value={app.activity} testid={`fg-activity-${serial}`} />
          <Field label={t("dashboard.foreground.pid")} value={app.pid} testid={`fg-pid-${serial}`} mono />
          <Field label={t("dashboard.foreground.nativeLib")} value={app.nativeLibDir} testid={`fg-libdir-${serial}`} mono />
          <div className="md:col-span-2">
            <p className="mb-1 text-muted-foreground">{t("dashboard.foreground.procPaths")}</p>
            <ul className="space-y-1" data-testid={`fg-proc-paths-${serial}`}>
              {app.procPaths.length === 0 && (
                <li className="text-muted-foreground">{t("dashboard.foreground.noProc")}</li>
              )}
              {app.procPaths.map((p) => (
                <li key={p.path} className="flex items-baseline gap-2 font-mono">
                  <PathText value={p.path} className="shrink-0 text-foreground" />
                  <span
                    className={cn(
                      "min-w-0 flex-1 break-all",
                      p.readable ? "text-muted-foreground" : "text-amber-500",
                    )}
                    title={p.summary ?? undefined}
                  >
                    {p.readable ? p.summary || t("common.emptyValue") : t("dashboard.foreground.unreadable")}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </div>
  );
}

/** 包类别指示灯：绿=三方应用、黄=系统应用、红=未知（探测失败） */
function KindDot({ kind }: { kind?: string }) {
  const { t } = useI18n();
  const tone =
    kind === "third_party"
      ? "bg-emerald-500"
      : kind === "system"
        ? "bg-amber-500"
        : "bg-red-500";
  const label =
    kind === "third_party"
      ? t("devices.kind.thirdParty")
      : kind === "system"
        ? t("devices.kind.system")
        : t("devices.kind.unknown");
  return (
    <span className="flex shrink-0 items-center gap-1" title={label}>
      <span className={cn("h-1.5 w-1.5 rounded-full", tone)} />
      <span className="text-[10px] text-muted-foreground">{label}</span>
    </span>
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

/* 为了 M1 页面回归可测（`app/m1PageRegression.test.tsx`）而导出：内部仍按子 tab 使用。 */
export function FilesView({ serial }: { serial: string | null }) {
  const { t } = useI18n();
  /** 浏览历史（后退/前进/上一级）；`path` 是当前所在目录 */
  const nav = usePathHistory("/sdcard", serial);
  const path = nav.path;
  /** 选中待预览/查看元数据的文件（AR7.1：filesystem.stat + filesystem.preview） */
  const [selected, setSelected] = useState<string | null>(null);
  /** 写操作（增删改）的界面态：一次只开一个行内对话框，避免叠出说不清的状态 */
  type FileEditor = { kind: "mkdir" | "rename" | "chmod"; path: string; value: string };
  const [editor, setEditor] = useState<FileEditor | null>(null);
  const [pendingDelete, setPendingDelete] = useState<{ path: string; name: string; isDir: boolean; recursive: boolean } | null>(null);
  const [writeBusy, setWriteBusy] = useState(false);
  const [writeNotice, setWriteNotice] = useState<string | null>(null);
  const [writeError, setWriteError] = useState<string | null>(null);
  const { data, isLoading, error, refetch } = useQuery({
    queryKey: ["device", "ls", serial, path],
    queryFn: () => deviceApi.ls(serial!, path),
    enabled: !!serial,
  });
  const selectedPath = selected ? joinRemote(path, selected) : null;
  const stat = useQuery({
    queryKey: ["device", "file-stat", serial, selectedPath],
    queryFn: () => deviceApi.fileStat(serial!, selectedPath!),
    enabled: !!serial && !!selectedPath,
  });
  const preview = useQuery({
    queryKey: ["device", "file-preview", serial, selectedPath],
    queryFn: () => deviceApi.filePreview(serial!, selectedPath!, { maxBytes: 64 * 1024 }),
    enabled: !!serial && !!selectedPath,
  });

  /**
   * 写操作的统一入口（增删改）。
   *
   * 每条都刷一次列表并给一句"设备实际怎么样"的结论，而不是"请求发出去了"：
   * 目录可能被 FUSE 吃掉权限位、非空目录会被设备侧拒绝、删除必须复核后才发现没删掉。
   */
  const runWrite = async (label: string, fn: () => Promise<string>) => {
    if (!serial) return;
    setWriteBusy(true);
    setWriteError(null);
    setWriteNotice(null);
    try {
      setWriteNotice(await fn());
      void refetch();
    } catch (e) {
      setWriteError(`${label}: ${String((e as Error)?.message ?? e)}`);
    } finally {
      setWriteBusy(false);
    }
  };

  const applyMkdir = () => {
    const name = editor?.kind === "mkdir" ? editor.value.trim() : "";
    if (!name) return;
    const target = joinRemote(path, name);
    setEditor(null);
    void runWrite(t("devices.files.mkdir"), async () => {
      const result = await deviceApi.fsMkdir(serial!, target);
      return result.created
        ? t("devices.files.mkdirDone", { path: result.path })
        : t("devices.files.mkdirExists");
    });
  };

  const uploadHere = () => {
    void pickFile({ title: t("devices.files.upload") }).then((picked) => {
      if (!picked || !serial) return;
      const name = picked.split("/").pop() ?? "";
      if (!name) return;
      void runWrite(t("devices.files.upload"), async () => {
        await deviceApi.push(serial, picked, joinRemote(path, name));
        return t("devices.files.uploaded", { name });
      });
    });
  };

  const retrieve = (entry: string) => {
    void pickDirectory(t("devices.files.retrieve")).then((dir) => {
      if (!dir || !serial) return;
      void runWrite(t("devices.files.retrieve"), async () => {
        await deviceApi.pull(serial, joinRemote(path, entry), `${dir}/${entry}`);
        return t("devices.files.retrieved", { name: entry });
      });
    });
  };

  const applyRename = () => {
    if (!editor || editor.kind !== "rename" || !editor.value.trim()) return;
    const target = editor.value.trim();
    const to = target.startsWith("/") ? target : joinRemote(path, target);
    const from = editor.path;
    setEditor(null);
    void runWrite(t("devices.files.rename"), async () => {
      const result = await deviceApi.fsRename(serial!, from, to);
      return t("devices.files.renameDone", { to: result.to });
    });
  };

  const applyChmod = () => {
    if (!editor || editor.kind !== "chmod") return;
    const mode = Number.parseInt(editor.value.trim(), 8);
    if (!Number.isFinite(mode) || mode < 0 || mode > 0o777) {
      setWriteError(t("devices.files.modeLabel"));
      return;
    }
    const target = editor.path;
    setEditor(null);
    void runWrite(t("devices.files.chmod"), async () => {
      const result = await deviceApi.fsChmod(serial!, target, mode);
      return result.verified
        ? t("devices.files.chmodDone", { mode: result.mode_text })
        : t("devices.files.chmodNotVerified", { mode: result.mode_text });
    });
  };

  const confirmDelete = () => {
    if (!pendingDelete) return;
    const { path: target, recursive } = pendingDelete;
    setPendingDelete(null);
    void runWrite(t("devices.files.remove"), async () => {
      const result = await deviceApi.fsRemove(serial!, target, recursive);
      return t("devices.files.removeDone", { path: result.path, size: formatSize(result.freed_bytes) });
    });
  };
  if (!serial) return <Empty text="未选择设备" />;
  const actionButton = { size: "sm", variant: "ghost" } as const;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <PathBar nav={nav} onRefresh={() => void refetch()} />
      <div className="flex shrink-0 items-center gap-1">
        <Button
          {...actionButton}
          className="h-7"
          disabled={writeBusy}
          onClick={() => setEditor({ kind: "mkdir", path, value: "" })}
        >
          <FolderPlus className="h-3.5 w-3.5" />
          {t("devices.files.mkdir")}
        </Button>
        <Button {...actionButton} className="h-7" disabled={writeBusy} onClick={uploadHere}>
          <Upload className="h-3.5 w-3.5" />
          {t("devices.files.upload")}
        </Button>
        {writeBusy && <span className="text-xs text-muted-foreground">{t("devices.files.busy")}</span>}
      </div>
      {editor?.kind === "mkdir" && (
        <div className="flex shrink-0 items-center gap-2 rounded-lg border bg-muted/30 p-2 text-xs">
          <span className="shrink-0">{t("devices.files.mkdirInto", { path })}</span>
          <input
            autoFocus
            value={editor.value}
            onChange={(e) => setEditor({ ...editor, value: e.target.value })}
            onKeyDown={(e) => e.key === "Enter" && applyMkdir()}
            className="h-7 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono focus-visible:outline-none"
          />
          <Button size="sm" onClick={applyMkdir}>
            {t("devices.files.apply")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setEditor(null)}>
            {t("devices.files.cancel")}
          </Button>
        </div>
      )}
      {writeNotice && <p className="shrink-0 text-xs text-muted-foreground">{writeNotice}</p>}
      {writeError && <p className="shrink-0 text-xs text-destructive">{t("devices.files.failed", { error: writeError })}</p>}
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border">
        {isLoading && <Empty text="加载中…" />}
        {error && (
          <Empty text={`读取失败：${String((error as Error)?.message ?? error)}`} />
        )}
        {data && data.length === 0 && <Empty text="（空目录）" />}
        {data && (
          <ul className="divide-y text-xs">
            {data.map((f) => {
              const entryPath = joinRemote(path, f.name);
              return (
                <li key={f.name} className="flex items-center gap-2 px-3 py-1.5">
                  {f.isDir ? (
                    <button
                      type="button"
                      className="flex min-w-0 flex-1 items-center gap-2 text-left hover:underline"
                      onClick={() => {
                        setSelected(null);
                        nav.go(entryPath);
                      }}
                    >
                      <span className="break-all font-medium">{f.name}/</span>
                    </button>
                  ) : (
                    <button
                      type="button"
                      className={cn(
                        "min-w-0 flex-1 break-all text-left hover:underline",
                        selected === f.name && "font-medium text-foreground underline",
                      )}
                      title="查看元数据与受限预览"
                      onClick={() => setSelected(selected === f.name ? null : f.name)}
                    >
                      {f.name}
                    </button>
                  )}
                  {f.symlink && <span className="text-muted-foreground">→ {f.symlink}</span>}
                  {f.perms && (
                    <span className="hidden shrink-0 font-mono text-muted-foreground sm:inline">
                      {f.perms}
                    </span>
                  )}
                  <span className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                    {f.isDir ? "-" : formatSize(f.size)}
                  </span>
                  <span className="flex shrink-0 items-center gap-0.5 opacity-0 focus-within:opacity-100 hover:opacity-100">
                    {!f.isDir && (
                      <Button
                        {...actionButton}
                        className="h-6 w-6 px-0"
                        title={t("devices.files.retrieve")}
                        aria-label={t("devices.files.retrieve")}
                        disabled={writeBusy}
                        onClick={() => retrieve(f.name)}
                      >
                        <Download className="h-3 w-3" />
                      </Button>
                    )}
                    <Button
                      {...actionButton}
                      className="h-6 w-6 px-0"
                      title={t("devices.files.rename")}
                      aria-label={t("devices.files.rename")}
                      disabled={writeBusy}
                      onClick={() => setEditor({ kind: "rename", path: entryPath, value: f.name })}
                    >
                      <Pencil className="h-3 w-3" />
                    </Button>
                    <Button
                      {...actionButton}
                      className="h-6 w-6 px-0"
                      title={t("devices.files.chmod")}
                      aria-label={t("devices.files.chmod")}
                      disabled={writeBusy}
                      onClick={() =>
                        setEditor({
                          kind: "chmod",
                          path: entryPath,
                          value: permsToOctal(f.perms),
                        })
                      }
                    >
                      <KeyRound className="h-3 w-3" />
                    </Button>
                    <Button
                      {...actionButton}
                      className="h-6 w-6 px-0 text-destructive hover:text-destructive"
                      title={t("devices.files.remove")}
                      aria-label={t("devices.files.remove")}
                      disabled={writeBusy}
                      onClick={() =>
                        setPendingDelete({ path: entryPath, name: f.name, isDir: f.isDir, recursive: false })
                      }
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {selectedPath && (
        <div className="shrink-0 rounded-lg border p-3 text-xs">
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <span className="font-medium">{selected}</span>
            <PathText value={stat.data?.path ?? selectedPath} testid="files-stat-path" className="font-mono text-muted-foreground" />
          </div>
          {stat.isLoading && <p className="mt-1 text-muted-foreground">读取元数据…</p>}
          {stat.error && (
            <p className="mt-1 text-destructive">
              元数据读取失败：{String((stat.error as Error)?.message ?? stat.error)}
            </p>
          )}
          {stat.data && (
            <p className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground">
              <span className="font-mono">{stat.data.stat.mode_text}</span>
              <span>{stat.data.stat.kind}</span>
              <span>uid:gid {stat.data.stat.uid}:{stat.data.stat.gid}</span>
              <span>{formatSize(stat.data.stat.size)}</span>
              <span>mtime {formatEpoch(stat.data.stat.mtime_unix)}</span>
              <span className={stat.data.stat.readable ? undefined : "text-destructive"}>
                {stat.data.stat.readable ? "可读" : "不可读（权限不足，不代表内容为空）"}
              </span>
            </p>
          )}
          {preview.error && (
            <p className="mt-2 text-destructive">
              预览失败：{String((preview.error as Error)?.message ?? preview.error)}
            </p>
          )}
          {preview.data && (
            <div className="mt-2">
              <p className="text-muted-foreground">
                预览 {preview.data.returned_bytes} / {formatSize(preview.data.size)}
                {preview.data.offset > 0 && `（偏移 ${preview.data.offset}）`}
                {preview.data.truncated && "，已截断"}
                {preview.data.encoding === "hex" && "，二进制按 hex 显示"}
                {preview.data.detail && `（${preview.data.detail}）`}
              </p>
              <pre className="mt-1 max-h-48 overflow-auto rounded-md bg-muted/40 p-2 font-mono whitespace-pre-wrap break-all">
                {preview.data.encoding === "hex"
                  ? preview.data.hex
                  : preview.data.text}
              </pre>
            </div>
          )}
        </div>
      )}
      {editor && editor.kind !== "mkdir" && (
        <div className="flex shrink-0 items-center gap-2 rounded-lg border bg-muted/30 p-2 text-xs">
          <PathText value={editor.path} className="min-w-0 flex-1 truncate font-mono text-muted-foreground" />
          <input
            autoFocus
            value={editor.value}
            onChange={(e) => setEditor({ ...editor, value: e.target.value })}
            onKeyDown={(e) => {
              if (e.key !== "Enter") return;
              if (editor.kind === "rename") applyRename();
              else applyChmod();
            }}
            placeholder={editor.kind === "rename" ? t("devices.files.newNameLabel") : t("devices.files.modeLabel")}
            className="h-7 w-56 shrink-0 rounded-md border border-input bg-transparent px-2 font-mono focus-visible:outline-none"
          />
          <Button
            size="sm"
            onClick={() => (editor.kind === "rename" ? applyRename() : applyChmod())}
          >
            {t("devices.files.apply")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setEditor(null)}>
            {t("devices.files.cancel")}
          </Button>
        </div>
      )}
      {pendingDelete && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-2 text-xs">
          <span className="font-medium text-destructive">
            {t("devices.files.confirmRemove", { name: pendingDelete.name })}
          </span>
          <PathText value={pendingDelete.path} className="min-w-0 flex-1 truncate font-mono text-muted-foreground" />
          {pendingDelete.isDir && (
            <label className="flex shrink-0 items-center gap-1">
              <input
                type="checkbox"
                checked={pendingDelete.recursive}
                onChange={(e) => setPendingDelete({ ...pendingDelete, recursive: e.target.checked })}
              />
              {t("devices.files.recursive")}
            </label>
          )}
          <Button size="sm" variant="destructive" onClick={confirmDelete}>
            {t("devices.files.remove")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setPendingDelete(null)}>
            {t("devices.files.cancel")}
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * `-rw-r--r--` → `644`。列表里的权限串是设备侧 `ls` 同源渲染的字符串，
 * 拿它当默认值比瞎猜 644 诚实；解析不出来就返回空串，让界面逼用户显式填。
 */
function permsToOctal(perms?: string | null): string {
  if (!perms || perms.length < 10) return "";
  const bits = perms.slice(1, 10);
  if (/[^rwxstT-]/.test(bits)) return "";
  let out = "";
  for (let group = 0; group < 3; group += 1) {
    const triplet = bits.slice(group * 3, group * 3 + 3);
    let value = 0;
    if (triplet[0] !== "-") value += 4;
    if (triplet[1] !== "-") value += 2;
    if (triplet[2] !== "-" && triplet[2] !== "T" && triplet[2] !== "S") value += 1;
    out += String(value);
  }
  return out;
}

/**
 * 与后端 `apk_bundle::is_bundle_path` 同一条判定：后缀 `.apks`，大小写不敏感。
 * 界面只用它决定要不要提示"这是容器"，真正的解包判断在后端。
 */
function isApksPath(path: string): boolean {
  const name = path.trim().toLowerCase();
  return name.endsWith(".apks");
}

export function AppsView({ serial }: { serial: string | null }) {
  const { t } = useI18n();
  const [selectedApp, setSelectedApp] = useState<ZygiskAppItem | null>(null);
  /**
   * 待安装文件：**只有一个路径**（AR8.2 的"多选 base + split"入口已收掉）。
   * 分包应用的正确载体是 `.apks` 容器，由后端读开后整套装，用户不需要懂 split 概念。
   */
  const [apkPath, setApkPath] = useState("");
  const [context, setContext] = useState<{ x: number; y: number; app: ZygiskAppItem } | null>(null);
  const [exporting, setExporting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [scope, setScope] = useState<ZygiskScope>("all");
  const [includeDisabled, setIncludeDisabled] = useState(false);
  const { data, isLoading, error, refetch } = useQuery({
    queryKey: ["zygisk", "applist", serial, scope, includeDisabled],
    queryFn: () => zygiskApi.list(serial!, scope, { includeDisabled }),
    enabled: !!serial,
  });
  // 清单失败时再取一次模块生命周期，区分「Agent 未连接 / 未安装 / 未启用 / 需重启」
  const { data: diagnosis } = useQuery({
    queryKey: ["zygisk", "status", serial],
    queryFn: () => zygiskApi.status(serial!),
    enabled: !!serial && !!error,
  });
  const [action, setAction] = useState<{ kind: string; taskId: string } | null>(null);
  /**
   * 包写操作（启动/强停/卸载）走 Agent typed 结果，**不再有任务卡**（AR8.1 收尾 + D041）。
   * 这里存的是「一眼能看懂的结论 + 设备侧步骤链」；`replayed`/`noOp`/`verified=false`
   * 三态必须与 `executed` 区分开，否则幂等命中会被显示成一次新的成功。
   */
  const [writeResult, setWriteResult] = useState<PackageWriteView | null>(null);

  if (!serial) return <Empty text={t("apps.noDevice")} />;
  if (isLoading) return <Empty text={t("apps.loading")} />;
  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-xs">
        <p className="font-medium text-destructive">{t("apps.unavailable")}</p>
        <p className="max-w-lg text-muted-foreground">{String((error as Error)?.message ?? error)}</p>
        <p className="max-w-lg text-muted-foreground">
          {diagnosis
            ? t("apps.lifecycle", {
                lifecycle: diagnosis.lifecycle,
                detail: diagnosis.detail ?? t("apps.unknown"),
              })
            : t("apps.unavailableHint")}
        </p>
        <Button size="sm" variant="outline" onClick={() => void refetch()}>
          <RefreshCw className="h-3.5 w-3.5" />
          {t("apps.retry")}
        </Button>
      </div>
    );
  }

  const apps = data?.items ?? [];
  const warnings = data?.warnings ?? [];

  const runWrite = async (
    fn: () => Promise<PackageWriteResult | PackageUninstallResult>,
    kind: string,
    done: (result: PackageWriteResult | PackageUninstallResult) => string,
    onDone?: (result: PackageWriteResult | PackageUninstallResult) => void,
  ) => {
    setAction(null);
    try {
      const result = await fn();
      onDone?.(result);
      setWriteResult({
        kind,
        outcome: result.outcome,
        verified: result.verified,
        summary: done(result),
        steps: "steps" in result ? result.steps : [],
        detail: result.detail,
      });
    } catch (e) {
      setWriteResult({
        kind,
        outcome: "executed",
        verified: false,
        summary: String((e as Error)?.message ?? e),
        steps: [],
        failed: true,
      });
    }
  };

  /**
   * 卸载成功（或幂等命中）后要自己刷清单：以前这条链靠任务中心的 `adb.uninstall`
   * 完成事件触发（TasksPage 里那个 kind 判断），现在不产卡了就得由发起方负责。
   * push/install 那两条仍然产卡，仍走 TasksPage 的既有通知。
   */
  const runAction = async (fn: () => Promise<string>, kind: string) => {
    try {
      const id = await fn();
      setAction({ kind, taskId: id });
    } catch (e) {
      setAction({ kind: kind + " 失败: " + String((e as Error).message ?? e), taskId: "" });
    }
  };

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setNotice(t("apps.copied", { field: label }));
    } catch (e) {
      setNotice(t("apps.copyFailed", { error: String((e as Error)?.message ?? e) }));
    }
    setContext(null);
  };

  const saveApk = async (app: ZygiskAppItem) => {
    setContext(null);
    const destination = await pickDirectory(t("apps.pickDestination"));
    if (!destination) return;
    setExporting(true);
    setNotice(null);
    try {
      const report = await zygiskApi.exportPackage(serial, app.packageName, destination);
      const base =
        report.kind === "apks"
          ? t("apps.savedApks", {
              parts: report.parts.length,
              name: report.fileName,
              size: formatSize(report.artifactBytes),
            })
          : t("apps.savedApk", { name: report.fileName, size: formatSize(report.artifactBytes) });
      setNotice(report.complete ? base : `${base} ${t("apps.savedIncomplete", { count: report.skipped.length })}`);
    } catch (e) {
      setNotice(t("apps.saveFailed", { error: String((e as Error)?.message ?? e) }));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="relative flex h-full min-h-0 gap-4" onClick={() => setContext(null)}>
      <div className="flex min-h-0 w-80 shrink-0 flex-col gap-2">
        <div className="flex shrink-0 items-center gap-2">
          <span className="text-xs font-medium">{t("apps.zygiskList")}</span>
          <span className="text-[11px] text-muted-foreground">{t("apps.count", { count: apps.length })}</span>
          {data && data.channel === "zygisk_v2" && (
            <span className="shrink-0 rounded bg-primary/10 px-1 text-[10px] text-primary">v2</span>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-7 px-2"
            disabled={exporting}
            onClick={(event) => {
              event.stopPropagation();
              void refetch();
            }}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("apps.refresh")}
          </Button>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {(["all", "user", "system"] as const).map((value) => (
            <button
              key={value}
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                setScope(value);
              }}
              className={cn(
                "rounded-full border px-2 py-0.5 text-[11px] hover:bg-accent",
                scope === value ? "border-primary text-foreground" : "text-muted-foreground",
              )}
            >
              {value === "all" ? t("apps.scopeAll") : value === "user" ? t("apps.scopeUser") : t("apps.scopeSystem")}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            setIncludeDisabled((v) => !v);
          }}
          className={cn(
            "w-fit shrink-0 rounded-full border px-2 py-0.5 text-[11px] hover:bg-accent",
            includeDisabled ? "border-primary text-foreground" : "text-muted-foreground",
          )}
        >
          {t("apps.includeDisabled")}
        </button>
        {data && data.fallbackCount > 0 && (
          <p className="shrink-0 text-[11px] text-muted-foreground">
            {t("apps.fallbackCount", { count: data.fallbackCount })}
            {data.deviceLocale ? ` · ${t("apps.deviceLocale", { locale: data.deviceLocale })}` : ""}
          </p>
        )}
        {data && data.channel !== "zygisk_v2" && (
          <p className="shrink-0 text-[11px] text-destructive">
            {t("apps.channelFallback", { channel: data.channel })}
          </p>
        )}
        {warnings.length > 0 && (
          <p className="shrink-0 truncate text-[11px] text-muted-foreground" title={warnings.map((w) => w.message).join(" / ")}>
            {warnings[0].message}
          </p>
        )}
        <div className="min-h-0 flex-1 overflow-auto rounded-lg border">
          {apps.length === 0 && <Empty text={t("apps.empty")} />}
          <ul className="text-xs">
            {apps.map((app) => (
            <li
              key={app.packageName}
              onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                setSelectedApp(app);
                setContext({ x: event.clientX, y: event.clientY, app });
              }}
            >
              <button
                type="button"
                onClick={(event) => {
                  event.stopPropagation();
                  setSelectedApp(app);
                  setContext(null);
                }}
                className={cn(
                  "block w-full px-3 py-2 text-left hover:bg-accent",
                  app.packageName === selectedApp?.packageName && "bg-accent font-medium",
                )}
              >
                <span className="flex items-center gap-1.5">
                  <span className="min-w-0 flex-1 truncate">{app.label || app.packageName}</span>
                  <span
                    className={cn(
                      "shrink-0 rounded px-1 text-[10px]",
                      app.isSystem ? "bg-muted text-muted-foreground" : "bg-primary/10 text-primary",
                    )}
                  >
                    {app.isSystem ? t("apps.badgeSystem") : t("apps.badgeUser")}
                  </span>
                  {!app.enabled && (
                    <span className="shrink-0 rounded bg-destructive/10 px-1 text-[10px] text-destructive">
                      {t("apps.badgeDisabled")}
                    </span>
                  )}
                  {app.labelSource === "package_name" && (
                    <span className="shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground">
                      {t("apps.badgeNoLabel")}
                    </span>
                  )}
                </span>
                <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
                  {app.packageName}
                </span>
              </button>
            </li>
          ))}
          </ul>
        </div>
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="rounded-lg border p-3 text-xs">
          <div className="font-medium">{selectedApp?.label ?? t("apps.noSelection")}</div>
          <div className="mt-1 break-all font-mono text-muted-foreground">
            {selectedApp?.packageName ?? t("apps.contextHint")}
          </div>
          {selectedApp && (
            <>
              <div className="mt-1 text-muted-foreground">
                {t("apps.version", {
                  version: selectedApp.versionName || t("apps.unknown"),
                  code: selectedApp.versionCode ?? t("apps.unknown"),
                })}
              </div>
              <div className="mt-1 text-muted-foreground">
                {t("apps.labelSource", {
                  source: selectedApp.labelSource,
                  locale: selectedApp.resolvedLocale ?? selectedApp.requestedLocale,
                })}
              </div>
              {selectedApp.fallbackReason && (
                <div className="mt-1 text-muted-foreground">
                  {t("apps.fallbackReason", { reason: selectedApp.fallbackReason })}
                </div>
              )}
              {selectedApp.uid !== null && (
                <div className="mt-1 font-mono text-muted-foreground">uid {selectedApp.uid}</div>
              )}
            </>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={!selectedApp}
            onClick={() =>
              selectedApp &&
              void runWrite(
                () => deviceApi.launch(serial, selectedApp.packageName),
                "启动",
                (r) =>
                  "pid" in r && r.pid
                    ? `${selectedApp.packageName} 已启动 · pid ${r.pid}`
                    : `${selectedApp.packageName} 启动命令已执行`,
              )
            }
          >
            <Rocket className="h-3.5 w-3.5" />
            启动
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!selectedApp}
            onClick={() =>
              selectedApp &&
              void runWrite(
                () => deviceApi.forceStop(serial, selectedApp.packageName),
                "强停",
                () => `${selectedApp.packageName} 已强制停止（复核到进程消失）`,
              )
            }
          >
            <CircleStop className="h-3.5 w-3.5" />
            强停
          </Button>
          <Button
            size="sm"
            variant="destructive"
            disabled={!selectedApp}
            onClick={() =>
              selectedApp &&
              void runWrite(
                () => deviceApi.uninstall(serial, selectedApp.packageName),
                "卸载",
                (r) =>
                  `${selectedApp.packageName} 已卸载 · 数据${"keep_data" in r && r.keep_data ? "保留" : "一并清除"}`,
                // 卸成功（含幂等命中）就自己刷清单：以前这条链靠任务完成事件，现在没有卡了
                (r) => {
                  if (r.outcome !== "executed" || r.verified) void refetch();
                },
              )
            }
          >
            <PackageOpen className="h-3.5 w-3.5" />
            卸载
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <input
            value={apkPath}
            onChange={(e) => setApkPath(e.target.value)}
            placeholder={t("devices.apkInstall.pathPlaceholder")}
            className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          />
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              void pickFile({
                title: t("devices.apkInstall.pickTitle"),
                filters: [{ name: "APK / APKS (*.apk, *.apks)", extensions: ["apk", "apks"] }],
              }).then((picked) => {
                if (picked) setApkPath(picked);
              })
            }
          >
            <PackageOpen className="h-3.5 w-3.5" />
            {t("devices.apkInstall.pick")}
          </Button>
          <Button
            size="sm"
            disabled={apkPath.trim() === ""}
            onClick={() =>
              void runAction(
                () => deviceApi.install(serial, [apkPath.trim()]),
                t("devices.apkInstall.run"),
              )
            }
          >
            <Upload className="h-3.5 w-3.5" />
            {t("devices.apkInstall.run")}
          </Button>
        </div>
        {isApksPath(apkPath) && (
          <p className="text-xs text-muted-foreground">{t("devices.apkInstall.bundleHint")}</p>
        )}
        {notice && <p className="text-xs text-muted-foreground">{notice}</p>}
        <div className="min-h-0 flex-1">
          {action ? (
            action.taskId ? (
              <TaskLaunchInline kind={action.kind} taskId={action.taskId} />
            ) : (
              <p className="text-xs text-destructive">{action.kind}</p>
            )
          ) : writeResult ? (
            <div
              className="rounded-md border px-2 py-1.5 text-xs"
              data-testid="package-write-result"
            >
              <p
                className={
                  writeResult.failed || (writeResult.outcome === "executed" && !writeResult.verified)
                    ? "font-medium text-destructive"
                    : writeResult.outcome === "executed"
                      ? "font-medium text-emerald-600 dark:text-emerald-400"
                      : "font-medium text-amber-500"
                }
              >
                {writeResult.kind} · {writeResult.summary}
              </p>
              {/* 幂等命中与「本来就在期望状态」不能显示成一次新的成功 */}
              {writeResult.outcome === "replayed" && (
                <p className="mt-1 text-[11px] text-amber-500">
                  幂等命中：这个操作刚刚已经做过，本次没有再动设备
                </p>
              )}
              {writeResult.outcome === "no_op" && (
                <p className="mt-1 text-[11px] text-muted-foreground">目标本来就在期望状态</p>
              )}
              {writeResult.outcome === "executed" && !writeResult.verified && !writeResult.failed && (
                <p className="mt-1 text-[11px] text-destructive">
                  命令执行了，但设备侧没复核到预期变化——别当成成功
                </p>
              )}
              {writeResult.steps.length > 0 && (
                <ul className="mt-1 space-y-0.5">
                  {writeResult.steps.map((step, index) => (
                    <li
                      key={`${step.name}-${index}`}
                      className="flex items-start gap-1.5 font-mono text-[11px]"
                      data-testid={`package-write-step-${step.name}`}
                    >
                      <span className={step.ok ? "text-emerald-600" : "text-destructive"}>
                        {step.ok ? "\u2713" : "\u2715"}
                      </span>
                      <span className="shrink-0">{step.name}</span>
                      {step.detail && (
                        <span className="min-w-0 break-all text-muted-foreground">
                          {step.detail}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ) : (
            <div className="flex h-full items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground">
              操作后此处显示结果（安装仍为任务，启动/强停/卸载为设备侧复核结果）
            </div>
          )}
        </div>
      </div>
      {context && (
        <div
          className="fixed z-50 min-w-44 rounded-md border bg-popover p-1 text-xs shadow-lg"
          style={{ left: context.x, top: context.y }}
          onClick={(event) => event.stopPropagation()}
        >
          <button
            type="button"
            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-accent"
            onClick={() => void copy(context.app.label, t("apps.appName"))}
          >
            <Copy className="h-3.5 w-3.5" />
            {t("apps.copyAppName")}
          </button>
          <button
            type="button"
            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-accent"
            onClick={() => void copy(context.app.packageName, t("apps.packageName"))}
          >
            <Copy className="h-3.5 w-3.5" />
            {t("apps.copyPackageName")}
          </button>
          <button
            type="button"
            disabled={exporting}
            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-accent disabled:opacity-50"
            onClick={() => void saveApk(context.app)}
          >
            <Save className="h-3.5 w-3.5" />
            {t("apps.saveApk")}
          </button>
        </div>
      )}
    </div>
  );
}

/** 一次性操作（安装/卸载/启动等）的输出面板：直接挂流订阅 + 历史回退 */
function TaskLaunchInline({ kind, taskId }: { kind: string; taskId: string }) {
  return <TaskSessionView key={taskId} label={kind} taskId={taskId} />;
}

function Empty({ text }: { text: string }) {
  return (
    <div className="flex h-full min-h-[100px] items-center justify-center text-xs text-muted-foreground">
      {text}
    </div>
  );
}

/** 设备侧路径拼接（与 Rust join_remote_path 同规则的 TS 版，仅 UI 导航用） */
function joinRemote(dir: string, name: string): string {
  const clean = (p: string) => p.split("/").filter((s) => s && s !== "..");
  const segs = [...clean(dir), ...clean(name)];
  return `/${segs.join("/")}`;
}

/** 文件大小人类可读（B/KB/MB/GB） */
/** Unix epoch 秒 → 本地时间（协议固定单位，前端只做展示格式化） */
function formatEpoch(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "-";
  return new Date(seconds * 1000).toLocaleString();
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes;
  let i = -1;
  do {
    v /= 1024;
    i++;
  } while (v >= 1024 && i < units.length - 1);
  return `${v.toFixed(v >= 100 ? 0 : 1)}${units[i]}`;
}
