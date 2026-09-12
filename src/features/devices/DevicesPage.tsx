import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw, Smartphone, PackageOpen, Rocket, CircleStop, Download, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SubTabs } from "@/components/nav/SubTabs";
import { TaskLaunchPanel } from "@/components/task/TaskLaunchPanel";
import { TaskSessionView } from "@/components/task/TaskSessionView";
import { deviceApi, type DeviceEntry } from "@/api/device";
import { envApi } from "@/api/env";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { PathText } from "@/components/ui/PathText";
import { BrandIcon } from "@/components/ui/BrandIcon";
import { cn } from "@/lib/utils";

/**
 * 设备页（P3）：设备列表/信息/Shell/文件/应用/Logcat 六个分 tab。
 * 长操作（shell/logcat/install/uninstall/push/pull）一律走 TaskService 任务，
 * 内联展示实时输出；设备热插拔由后端 watch 线程事件驱动刷新。
 */
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
              <p className="truncate font-medium">{d.model || "未知型号"}</p>
              <p className="truncate font-mono text-muted-foreground">{d.serial}</p>
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

  const rows: [string, string | undefined][] = [
    [t("devices.info.model"), data?.model],
    [t("devices.info.manufacturer"), data?.manufacturer],
    [t("devices.info.androidVersion"), data?.androidVersion],
    [t("devices.info.sdk"), data?.sdkInt],
    [t("devices.info.serial"), serial],
    [t("devices.info.transport"), transport],
  ];

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

      {/* 分割线 */}
      <hr className="my-4 border-border" />

      {/* 下：前台应用（原仪表盘信息，按设备查询） */}
      <ForegroundAppSection serial={serial} />
    </div>
  );
}

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
          <Field label={t("dashboard.foreground.package")} value={app.package} testid={`fg-package-${serial}`} />
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
                      "min-w-0 flex-1 truncate",
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

function FilesView({ serial }: { serial: string | null }) {
  const [path, setPath] = useState("/sdcard");
  const { data, isLoading, error, refetch } = useQuery({
    queryKey: ["device", "ls", serial, path],
    queryFn: () => deviceApi.ls(serial!, path),
    enabled: !!serial,
  });

  if (!serial) return <Empty text="未选择设备" />;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 items-center gap-2">
        <input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void refetch()}
          className="h-8 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        />
        <Button size="sm" variant="outline" onClick={() => void refetch()}>
          <RefreshCw className="h-3.5 w-3.5" />
          进入
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border">
        {isLoading && <Empty text="加载中…" />}
        {error && (
          <Empty text={`读取失败：${String((error as Error)?.message ?? error)}`} />
        )}
        {data && data.length === 0 && <Empty text="（空目录）" />}
        {data && (
          <ul className="divide-y text-xs">
            {data.map((f) => (
              <li key={f.name} className="flex items-center gap-2 px-3 py-1.5">
                {f.isDir ? (
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-center gap-2 text-left hover:underline"
                    onClick={() =>
                      setPath(joinRemote(path, f.name))
                    }
                  >
                    <span className="truncate font-medium">{f.name}/</span>
                  </button>
                ) : (
                  <span className="min-w-0 flex-1 truncate">{f.name}</span>
                )}
                {f.symlink && <span className="text-muted-foreground">→ {f.symlink}</span>}
                <span className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                  {f.isDir ? "-" : formatSize(f.size)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="flex shrink-0 gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled
          title="本地路径选择将随 P7 原生文件对话框接入"
        >
          <Upload className="h-3.5 w-3.5" />
          Push…
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled
          title="本地保存路径选择将随 P7 原生文件对话框接入"
        >
          <Download className="h-3.5 w-3.5" />
          Pull…
        </Button>
        <p className="self-center text-xs text-muted-foreground">
          传输任务接口已就绪（device_push/pull），按钮待文件选择器
        </p>
      </div>
    </div>
  );
}

function AppsView({ serial }: { serial: string | null }) {
  const [selectedPkg, setSelectedPkg] = useState<string | null>(null);
  const [apkPath, setApkPath] = useState("");
  const { data, isLoading, error } = useQuery({
    queryKey: ["device", "packages", serial],
    queryFn: () => deviceApi.packages(serial!),
    enabled: !!serial,
  });
  const [action, setAction] = useState<{ kind: string; taskId: string } | null>(null);

  if (!serial) return <Empty text="未选择设备" />;
  if (isLoading) return <Empty text="加载应用列表…" />;
  if (error) return <Empty text={`加载失败：${String((error as Error)?.message ?? error)}`} />;

  const runAction = async (fn: () => Promise<string>, kind: string) => {
    try {
      const id = await fn();
      setAction({ kind, taskId: id });
    } catch (e) {
      setAction({ kind: `${kind} 失败: ${String((e as Error).message ?? e)}`, taskId: "" });
    }
  };

  return (
    <div className="flex h-full min-h-0 gap-4">
      <div className="min-h-0 w-64 shrink-0 overflow-auto rounded-lg border">
        {data!.length === 0 && <Empty text="无第三方应用" />}
        <ul className="text-xs">
          {data!.map((pkg) => (
            <li key={pkg}>
              <button
                type="button"
                onClick={() => setSelectedPkg(pkg)}
                className={cn(
                  "block w-full truncate px-3 py-1.5 text-left hover:bg-accent",
                  pkg === selectedPkg && "bg-accent font-medium",
                )}
              >
                {pkg}
              </button>
            </li>
          ))}
        </ul>
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={!selectedPkg}
            onClick={() =>
              selectedPkg && void runAction(() => deviceApi.launch(serial, selectedPkg), "启动")
            }
          >
            <Rocket className="h-3.5 w-3.5" />
            启动
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!selectedPkg}
            onClick={() =>
              selectedPkg &&
              void runAction(() => deviceApi.forceStop(serial, selectedPkg), "强停")
            }
          >
            <CircleStop className="h-3.5 w-3.5" />
            强停
          </Button>
          <Button
            size="sm"
            variant="destructive"
            disabled={!selectedPkg}
            onClick={() =>
              selectedPkg &&
              void runAction(() => deviceApi.uninstall(serial, selectedPkg), "卸载")
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
            placeholder="本机 APK 路径（P7 接原生选择器）"
            className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          />
          <Button
            size="sm"
            disabled={!apkPath.trim()}
            onClick={() => void runAction(() => deviceApi.install(serial, apkPath.trim()), "安装")}
          >
            <Upload className="h-3.5 w-3.5" />
            安装
          </Button>
        </div>
        <div className="min-h-0 flex-1">
          {action ? (
            action.taskId ? (
              <TaskLaunchInline kind={action.kind} taskId={action.taskId} />
            ) : (
              <p className="text-xs text-destructive">{action.kind}</p>
            )
          ) : (
            <div className="flex h-full items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground">
              操作后此处显示任务输出
            </div>
          )}
        </div>
      </div>
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
