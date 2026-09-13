import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Play, RefreshCw, ShieldCheck, Square, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type DeviceEntry, type HostedBinary, type ListenPort } from "@/api/device";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * 二进制托管（ADB 页子标签）：管理 /data/local/tmp 下的 ELF 文件。
 * - 上区：file 判 ELF 后列出——绿色 = 有执行权限（双击加入下区托管），
 *   红色 = 无执行权限（「赋予权限」按钮走 chmod +x）；
 * - 下区：托管清单——「执行」后台启动（cd 目录 + nohup ./name）并回显 pid，
 *   同时拉取该 pid 的 LISTEN 端口（/proc/<pid>/fd → /proc/net/tcp(6)），
 *   pid 与端口 chip 单击复制；有 pid 时「终止」按钮走 kill -9；可移除托管行。
 * - Root 开关：勾选时先 `su -c id` 探测，可用才开——chmod/启动/存活复查/
 *   kill/日志读取整链路走 su -c（root 进程 shell 用户连 kill -0 都会 EPERM）。
 * 所有 adb 调用后端 -s 绑定设备。
 */

interface HostedRow {
  name: string;
  pid: number | null;
  running: boolean;
  error: string | null;
  /** 启动时所用的 root 上下文：kill/后续操作必须同身份 */
  root: boolean;
  ports: ListenPort[];
  /** 端口查询进行中标记 */
  portsLoading: boolean;
}

export function BinaryHosting() {
  const { t } = useI18n();
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [hosted, setHosted] = useState<HostedRow[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [root, setRoot] = useState(false);
  const [probing, setProbing] = useState(false);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const adbReady = !!env?.installed;

  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "binary"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);
  useEffect(() => {
    if (!deviceSerial && online.length > 0) setDeviceSerial(online[0].serial);
  }, [deviceSerial, online]);

  const {
    data: binaries = [],
    isLoading,
    isError,
    error: listError,
    refetch,
    isFetching,
  } = useQuery({
    queryKey: ["device", "binaries", deviceSerial],
    queryFn: () => deviceApi.binaries(deviceSerial!),
    enabled: !!deviceSerial,
    retry: false,
  });

  // 设备切换：托管区清空（pid 属于旧设备）；Root 探测状态失效
  useEffect(() => {
    setHosted([]);
    setNotice(null);
    setRoot(false);
  }, [deviceSerial]);

  const patchRow = (name: string, p: Partial<HostedRow>) =>
    setHosted((hs) => hs.map((h) => (h.name === name ? { ...h, ...p } : h)));

  const addHosted = (b: HostedBinary) => {
    if (!b.hasExec) return; // 红色不可双击托管（先 chmod）
    setHosted((hs) =>
      hs.some((h) => h.name === b.name)
        ? hs
        : [
            ...hs,
            {
              name: b.name,
              pid: null,
              running: false,
              error: null,
              root: false,
              ports: [],
              portsLoading: false,
            },
          ],
    );
  };

  /** 拉取某进程 LISTEN 端口（执行后自动调用；也供手动刷新） */
  const loadPorts = useCallback(
    async (name: string, pid: number, asRoot: boolean) => {
      if (!deviceSerial) return;
      patchRow(name, { portsLoading: true });
      try {
        const ports = await deviceApi.binaryPorts(deviceSerial, pid, asRoot);
        patchRow(name, { ports, portsLoading: false });
      } catch (e) {
        patchRow(name, { ports: [], portsLoading: false });
        setNotice(String((e as Error)?.message ?? e));
      }
    },
    [deviceSerial],
  );

  /** 勾选 Root：先 su -c id 探测；不可用则不开启并提示 */
  const toggleRoot = async (checked: boolean) => {
    if (!checked) {
      setRoot(false);
      return;
    }
    if (!deviceSerial) return;
    setProbing(true);
    try {
      const ok = await deviceApi.binarySuCheck(deviceSerial);
      if (ok) {
        setRoot(true);
        setNotice(t("adb.binary.suOk"));
      } else {
        setNotice(t("adb.binary.suFail"));
      }
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setProbing(false);
    }
  };

  const chmod = async (b: HostedBinary) => {
    if (!deviceSerial) return;
    try {
      await deviceApi.binaryChmod(deviceSerial, b.name, root);
      setNotice(t("adb.binary.chmodOk", { name: b.name }));
      void refetch();
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  const run = async (row: HostedRow) => {
    if (!deviceSerial || row.running) return;
    // 启动身份 = 点击瞬间的 Root 开关；写回本行，kill/复查永远同链路
    const asRoot = root;
    patchRow(row.name, { running: true, error: null, root: asRoot });
    try {
      const pid = await deviceApi.binaryRun(deviceSerial, row.name, asRoot);
      patchRow(row.name, { pid, running: false, ports: [], error: null });
      // 端口可能在 listen() 前几十毫秒才绑定：立即拉一次，3s 后再补一次
      void loadPorts(row.name, pid, asRoot);
      window.setTimeout(() => void loadPorts(row.name, pid, asRoot), 3000);
    } catch (e) {
      patchRow(row.name, { running: false, error: String((e as Error)?.message ?? e) });
    }
  };

  const kill = async (row: HostedRow) => {
    if (!deviceSerial || row.pid === null) return;
    patchRow(row.name, { running: true, error: null });
    try {
      await deviceApi.binaryKill(deviceSerial, row.pid, row.root);
      patchRow(row.name, {
        pid: null,
        running: false,
        ports: [],
        error: t("adb.binary.killed", { pid: row.pid }),
      });
    } catch (e) {
      patchRow(row.name, { running: false, error: String((e as Error)?.message ?? e) });
    }
  };

  if (!adbReady) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
        {env?.hint ?? t("common.loading")}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 items-center gap-3">
        <p className="text-xs leading-relaxed text-muted-foreground">{t("adb.binary.description")}</p>
      </div>

      <DeviceBar
        online={online}
        selected={deviceSerial}
        onSelect={setDeviceSerial}
        onRefresh={() => void refetch()}
        refreshing={isFetching}
        root={root}
        probing={probing}
        onRootChange={(c) => void toggleRoot(c)}
      />

      {/* 上区：ELF 文件列表 */}
      <section className="flex min-h-0 flex-1 flex-col gap-1" aria-label={t("adb.binary.listTitle")}>
        <div className="flex shrink-0 items-center justify-between">
          <h3 className="text-xs font-semibold">{t("adb.binary.listTitle")}</h3>
          <span className="text-[10px] text-muted-foreground">{t("adb.binary.listHint")}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded-lg border bg-card">
          {isLoading && <p className="p-3 text-xs text-muted-foreground">{t("common.loading")}</p>}
          {isError && (
            <p className="p-3 break-all text-xs leading-relaxed text-destructive">
              {String((listError as Error)?.message ?? listError)}
            </p>
          )}
          {!isLoading && !isError && binaries.length === 0 && (
            <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.empty")}</p>
          )}
          <ul className="divide-y text-xs">
            {binaries.map((b) => (
              <li key={b.name} data-testid={`bin-${b.name}`}>
                <button
                  type="button"
                  className={cn(
                    "flex w-full items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-accent",
                    !b.hasExec && "cursor-default hover:bg-transparent",
                  )}
                  onDoubleClick={() => addHosted(b)}
                  title={b.hasExec ? t("adb.binary.dblClickAdd") : undefined}
                >
                  <span
                    className={cn(
                      "min-w-0 flex-1 break-all font-mono font-medium",
                      b.hasExec ? "text-emerald-500" : "text-red-500",
                    )}
                  >
                    {b.name}
                  </span>
                  <span className="shrink-0 font-mono text-muted-foreground">{b.perms}</span>
                  <span className="w-14 shrink-0 text-right tabular-nums text-muted-foreground">
                    {b.size} B
                  </span>
                  {!b.hasExec && (
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-6 shrink-0 gap-1 px-2"
                      onClick={(e) => {
                        e.stopPropagation();
                        void chmod(b);
                      }}
                    >
                      <ShieldCheck className="h-3 w-3" />
                      {t("adb.binary.chmod")}
                    </Button>
                  )}
                </button>
              </li>
            ))}
          </ul>
        </div>
      </section>

      {/* 下区：托管执行 */}
      <section className="flex shrink-0 max-h-[45%] flex-col gap-1" aria-label={t("adb.binary.hostTitle")}>
        <div className="flex shrink-0 items-center justify-between">
          <h3 className="text-xs font-semibold">{t("adb.binary.hostTitle")}</h3>
          <span className="text-[10px] text-muted-foreground">{t("adb.binary.dblClickAdd")}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded-lg border bg-card">
          {hosted.length === 0 ? (
            <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.hostEmpty")}</p>
          ) : (
            <ul className="divide-y text-xs">
              {hosted.map((row) => {
                const bin = binaries.find((b) => b.name === row.name);
                return (
                  <li key={row.name} className="px-3 py-2" data-testid={`hosted-${row.name}`}>
                    <div className="flex items-center gap-3">
                      <InfoChip
                        className="min-w-0 flex-1 text-left"
                        label={`./${row.name}`}
                        title={t("adb.binary.copyCmd", { name: row.name })}
                        testid={`cmd-${row.name}`}
                      />
                      {row.pid !== null ? (
                        <InfoChip
                          label={`pid ${row.pid}${row.root ? " · root" : ""}`}
                          title={t("adb.binary.copyPid")}
                          testid={`pid-${row.name}`}
                        />
                      ) : (
                        <span className="shrink-0 text-muted-foreground">{t("adb.binary.idle")}</span>
                      )}
                      {row.pid !== null ? (
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-6 shrink-0 gap-1 px-2 text-destructive"
                          disabled={row.running}
                          onClick={() => void kill(row)}
                        >
                          <Square className="h-3 w-3" />
                          {t("adb.binary.kill")}
                        </Button>
                      ) : (
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-6 shrink-0 gap-1 px-2"
                          disabled={row.running || bin?.hasExec === false}
                          onClick={() => void run(row)}
                        >
                          {row.running ? <RefreshCw className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
                          {row.running ? t("adb.binary.starting") : t("adb.binary.execute")}
                        </Button>
                      )}
                      {row.pid !== null && (
                        <Button
                          size="sm"
                          variant="ghost"
                          className="h-6 shrink-0 px-1.5 text-muted-foreground"
                          disabled={row.portsLoading}
                          title={t("adb.binary.refreshPorts")}
                          onClick={() => row.pid !== null && void loadPorts(row.name, row.pid, row.root)}
                        >
                          <RefreshCw className={cn("h-3 w-3", row.portsLoading && "animate-spin")} />
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 shrink-0 px-1.5 text-muted-foreground"
                        disabled={row.pid !== null}
                        title={t("adb.binary.removeRow")}
                        onClick={() => setHosted((hs) => hs.filter((h) => h.name !== row.name))}
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </div>
                    {row.error && (
                      <p className="mt-1.5 break-all text-[11px] leading-relaxed text-destructive" data-testid={`err-${row.name}`}>
                        {row.error}
                      </p>
                    )}
                    {row.pid !== null && (
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5" data-testid={`ports-${row.name}`}>
                        {row.ports.length === 0 ? (
                          <span className="text-[10px] text-muted-foreground">
                            {row.portsLoading ? t("adb.binary.portsLoading") : t("adb.binary.noPorts")}
                          </span>
                        ) : (
                          row.ports.map((p) => {
                            const text = `${p.address}:${p.port}`;
                            return (
                              <InfoChip
                                key={`${p.family}-${text}`}
                                label={text}
                                title={t("adb.binary.copyPort", { family: p.family })}
                                testid={`port-${row.name}-${p.port}`}
                              />
                            );
                          })
                        )}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>

      {notice && (
        <p className="shrink-0 break-all rounded-md border bg-muted/40 px-2 py-1 text-xs text-muted-foreground" data-testid="binary-notice">
          {notice}
        </p>
      )}
    </div>
  );
}

/** 信息胶囊：纯文本可选中（path-selectable 单击全选/拖选/右键复制），非按钮 */
export function InfoChip({
  label,
  testid,
  title,
  className,
}: {
  label: string;
  testid: string;
  title?: string;
  className?: string;
}) {
  return (
    <span
      data-testid={testid}
      title={title}
      className={cn(
        "path-selectable max-w-full break-all rounded bg-emerald-500/10 px-1.5 py-0.5 font-mono text-emerald-500",
        className,
      )}
    >
      {label}
    </span>
  );
}

export function DeviceBar({
  online,
  selected,
  onSelect,
  onRefresh,
  refreshing,
  root,
  probing,
  onRootChange,
}: {
  online: DeviceEntry[];
  selected: string | null;
  onSelect: (s: string) => void;
  onRefresh: () => void;
  refreshing: boolean;
  root: boolean;
  probing: boolean;
  onRootChange: (checked: boolean) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex shrink-0 items-center gap-2 text-xs">
      {online.length === 0 && <span className="text-muted-foreground">{t("adb.forward.rowInactive")}</span>}
      {online.length === 1 && (
        <span className="font-mono text-muted-foreground">-s {online[0].serial}</span>
      )}
      {online.length > 1 && (
        <label className="flex items-center gap-2">
          <span className="font-mono text-muted-foreground">-s</span>
          <select
            className="h-7 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
            value={selected ?? ""}
            onChange={(e) => onSelect(e.target.value)}
          >
            {online.map((d) => (
              <option key={d.serial} value={d.serial}>
                {d.model || d.serial}（{d.serial}）
              </option>
            ))}
          </select>
        </label>
      )}
      <label
        className={cn(
          "ml-auto flex shrink-0 items-center gap-1.5 text-muted-foreground",
          probing && "opacity-50",
        )}
        title={t("adb.binary.rootHint")}
      >
        <input
          type="checkbox"
          aria-label={t("adb.binary.rootLabel")}
          disabled={probing || !selected}
          checked={root}
          onChange={(e) => onRootChange(e.target.checked)}
        />
        Root (su)
      </label>
      <Button
        size="sm"
        variant="outline"
        className="h-7 gap-1 px-2"
        disabled={refreshing || !selected}
        onClick={onRefresh}
      >
        <RefreshCw className={cn("h-3 w-3", refreshing && "animate-spin")} />
        {t("common.refresh")}
      </Button>
    </div>
  );
}
