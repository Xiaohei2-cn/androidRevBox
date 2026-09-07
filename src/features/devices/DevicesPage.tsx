import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw, Smartphone, PackageOpen, Rocket, CircleStop, Download, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SubTabs } from "@/components/nav/SubTabs";
import { TaskLaunchPanel } from "@/components/task/TaskLaunchPanel";
import { TaskSessionView } from "@/components/task/TaskSessionView";
import { deviceApi, type DeviceEntry } from "@/api/device";
import { cn } from "@/lib/utils";

/**
 * 设备页（P3）：设备列表/信息/Shell/文件/应用/Logcat 六个分 tab。
 * 长操作（shell/logcat/install/uninstall/push/pull）一律走 TaskService 任务，
 * 内联展示实时输出；设备热插拔由后端 watch 线程事件驱动刷新。
 */
export function DevicesPage() {
  const [selected, setSelected] = useState<string | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);

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
          content: <DeviceInfoView serial={activeSerial} />,
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
            <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              {d.transport}
            </span>
            <span
              className={cn(
                "w-20 text-right text-[10px]",
                d.state === "device" ? "text-emerald-500" : "text-amber-500",
              )}
            >
              {STATE_LABEL[d.state] ?? d.state}
            </span>
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

function DeviceInfoView({ serial }: { serial: string | null }) {
  const { data, isLoading, error } = useQuery({
    queryKey: ["device", "info", serial],
    queryFn: () => deviceApi.info(serial!),
    enabled: !!serial,
  });
  if (!serial) return <Empty text="未选择设备" />;
  if (isLoading) return <Empty text="读取属性中…" />;
  if (error)
    return <Empty text={`读取失败：${String((error as Error)?.message ?? error)}`} />;
  const rows: [string, string][] = [
    ["型号", data!.model],
    ["厂商", data!.manufacturer],
    ["Android 版本", data!.androidVersion],
    ["SDK", data!.sdkInt],
    ["序列号", data!.serial],
  ];
  return (
    <div className="max-w-md rounded-lg border bg-card p-4">
      <dl className="grid grid-cols-[110px_1fr] gap-y-2 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="break-all font-mono">{v || "—"}</dd>
          </div>
        ))}
      </dl>
    </div>
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
        <p className="self-center text-[10px] text-muted-foreground">
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
