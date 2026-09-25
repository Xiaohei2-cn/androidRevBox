import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Play, RefreshCw, ShieldCheck, Square, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AdbNotReadyState } from "@/components/ui/adb-gate";
import { deviceApi, type HostedBinary, type HostedRunRecord, type ListenPort } from "@/api/device";
import { DeviceBar } from "@/components/ui/device-bar";
import { InfoChip } from "@/components/ui/info-chip";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * 二进制托管（「二进制」主 tab 的子标签，第四十七轮从 ADB 页挪过来）：管理 /data/local/tmp 下的 ELF 文件。
 * - 上区：Agent `hosted.list` 列出托管目录里的 ELF（文件头 magic 判定，不依赖设备端
 *   `file` 命令）——绿色 = 有执行权限（双击加入下区托管），红色 = 无执行权限
 *   （「赋予权限」走 Agent `hosted.chmod`，只补执行位且幂等）；
 * - 下区：托管清单——「执行」走 Agent `hosted.start`（参数数组 exec，不进 shell），
 *   回显 pid 并给出稳定句柄；行状态由设备端运行表（`hosted.list` 的 runs，5s 轮询）
 *   校正，Desktop 重启或刷新后不丢；端口 chip 复用 Agent `process.ports`；
 * - 「终止」优先走 Agent `hosted.stop`（发信号前用落盘的 start time 复核身份，
 *   PID 易主时拒止而不是照数字杀，并回收自己启动的子进程拿到真实死因）；
 *   没有句柄时退回按 PID + 进程名的 `process.kill`。
 * - Root 开关：勾选时先 `su -c id` 探测，可用才开——chmod/启动/终止/日志读取整链路
 *   仍走 Legacy `su -c`（Agent 以 shell 身份运行，root 属主进程它碰不到）。
 * 所有 adb 调用后端 -s 绑定设备。
 */

interface HostedRow {
  name: string;
  /** Agent 运行表里的稳定句柄；有它才能按 handle + start time 停止（AR7.3） */
  handle: string | null;
  pid: number | null;
  running: boolean;
  error: string | null;
  /** 启动时所用的 root 上下文：kill/后续操作必须同身份 */
  root: boolean;
  ports: ListenPort[];
  /** 端口查询进行中标记 */
  portsLoading: boolean;
  /** 备注（用途说明），localStorage 持久化 */
  note: string;
}

/** 常驻快捷备注选项（值为写入备注的文本本身，跨语言固定） */
const NOTE_PRESETS = ["frida server", "ida远程调试server", "dumper"] as const;

/** 备注持久化键：设备 serial + 文件名维度，跨重启/重托管保留 */
const noteKey = (serial: string, name: string) => `adb.binary.note.${serial}.${name}`;
const loadNote = (serial: string | null, name: string) =>
  (serial && localStorage.getItem(noteKey(serial, name))) || "";
const saveNote = (serial: string, name: string, value: string) => {
  if (value) localStorage.setItem(noteKey(serial, name), value);
  else localStorage.removeItem(noteKey(serial, name));
};

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

  /**
   * Agent 侧托管运行表（AR7.2）：pid 与状态来自设备端 `pid + start time` 对账，
   * 不是前端本地记忆，所以 Desktop 重启、页面刷新后也能恢复「谁真的在跑」。
   */
  const { data: runs = [] } = useQuery({
    queryKey: ["adb", "hosted-runs", deviceSerial],
    queryFn: () => deviceApi.hostedRuns(deviceSerial!),
    enabled: !!deviceSerial,
    refetchInterval: 5_000,
    retry: false,
  });

  // 设备切换：托管区清空（pid 属于旧设备）；Root 探测状态失效
  useEffect(() => {
    setHosted([]);
    setNotice(null);
    setRoot(false);
  }, [deviceSerial]);

  // 用运行表校正/补齐托管行：设备端说在跑就是在跑，说退了就把状态收回去
  useEffect(() => {
    if (runs.length === 0) return;
    setHosted((rows) => {
      const next = rows.map((row) => {
        const mine = runs
          .filter((r: HostedRunRecord) => r.name === row.name)
          .sort((a, b) => b.started_at_unix - a.started_at_unix);
        const live = mine.find((r) => r.state === "running");
        if (live) {
          return {
            ...row,
            handle: live.handle,
            pid: live.pid,
            running: true,
            root: live.root,
          };
        }
        const last = mine[0];
        if (last && last.state === "exited" && row.running) {
          return { ...row, running: false, pid: last.pid };
        }
        return row;
      });
      const known = new Set(next.map((row) => row.name));
      for (const run of runs) {
        if (run.state !== "running" || known.has(run.name)) continue;
        next.push({
          name: run.name,
          handle: run.handle,
          pid: run.pid,
          running: true,
          error: null,
          root: run.root,
          ports: [],
          portsLoading: false,
          note: loadNote(deviceSerial, run.name),
        });
        known.add(run.name);
      }
      return next;
    });
  }, [runs, deviceSerial]);

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
              handle: null,
              pid: null,
              running: false,
              error: null,
              root: false,
              ports: [],
              portsLoading: false,
              note: loadNote(deviceSerial, b.name),
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

  /**
   * 终止托管进程。有句柄（AR7.3）时优先 `hosted.stop`：Agent 会用落盘的 start time
   * 复核身份，PID 易主时拒止而不是照数字杀，也会顺手回收自己启动的子进程拿到死因。
   * 没有句柄（Legacy/root 启动的进程、Agent 未连接）时退回按 PID + 进程名终止。
   */
  const kill = async (row: HostedRow) => {
    if (!deviceSerial || row.pid === null) return;
    patchRow(row.name, { running: true, error: null });
    try {
      if (row.handle && !row.root) {
        const stopped = await deviceApi.hostedStop(
          deviceSerial,
          row.handle,
          row.pid ?? undefined,
        );
        // 核过身份才算「确认杀的就是它」；未核过时把详情显示出来，不静默当成成功
        if (!stopped.identity_verified && stopped.outcome === "signaled") {
          patchRow(row.name, {
            pid: null,
            running: false,
            ports: [],
            error: t("adb.binary.stopUnverified", {
            name: row.name,
            detail: stopped.record.detail ?? "-",
          }),
          });
          return;
        }
      } else {
        await deviceApi.binaryKill(deviceSerial, row.pid, row.root, row.name);
      }
      patchRow(row.name, {
        handle: null,
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
    return <AdbNotReadyState hint={env?.hint} />;
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
          <span className="text-10px text-muted-foreground">{t("adb.binary.listHint")}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden rounded-xl border border-border/70 bg-card shadow-card">
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
                    "flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-accent",
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
                  {/* 权限/大小/操作固定列宽：无按钮行同位占格，右缘垂直对齐 */}
                  <span className="w-[78px] shrink-0 text-right font-mono text-muted-foreground">
                    {b.perms}
                  </span>
                  <span className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                    {b.size} B
                  </span>
                  <span className="flex h-6 w-[104px] shrink-0 items-center justify-end">
                    {!b.hasExec && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-6 gap-1 px-2"
                        onClick={(e) => {
                          e.stopPropagation();
                          void chmod(b);
                        }}
                      >
                        <ShieldCheck className="h-3 w-3" />
                        {t("adb.binary.chmod")}
                      </Button>
                    )}
                  </span>
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
          <span className="text-10px text-muted-foreground">{t("adb.binary.dblClickAdd")}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded-xl border border-border/70 bg-card shadow-card">
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
                    <div className="mt-1.5 flex items-center gap-1.5">
                      <input
                        aria-label={t("adb.binary.noteLabel", { name: row.name })}
                        data-testid={`note-${row.name}`}
                        className="path-selectable h-6 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-xs"
                        placeholder={t("adb.binary.notePlaceholder")}
                        value={row.note}
                        onChange={(e) => {
                          const v = e.target.value;
                          patchRow(row.name, { note: v });
                          if (deviceSerial) saveNote(deviceSerial, row.name, v);
                        }}
                      />
                      <select
                        aria-label={t("adb.binary.noteQuick")}
                        data-testid={`note-quick-${row.name}`}
                        className="h-6 shrink-0 rounded-md border border-input bg-transparent px-1 text-xs text-muted-foreground"
                        value=""
                        onChange={(e) => {
                          if (!e.target.value) return;
                          patchRow(row.name, { note: e.target.value });
                          if (deviceSerial) saveNote(deviceSerial, row.name, e.target.value);
                          e.target.value = "";
                        }}
                      >
                        <option value="">{t("adb.binary.noteQuick")}</option>
                        {NOTE_PRESETS.map((preset) => (
                          <option key={preset} value={preset}>
                            {preset}
                          </option>
                        ))}
                      </select>
                    </div>
                    {row.error && (
                      <p className="mt-1.5 break-all text-11px leading-relaxed text-destructive" data-testid={`err-${row.name}`}>
                        {row.error}
                      </p>
                    )}
                    {row.pid !== null && (
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5" data-testid={`ports-${row.name}`}>
                        {row.ports.length === 0 ? (
                          <span className="text-10px text-muted-foreground">
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
