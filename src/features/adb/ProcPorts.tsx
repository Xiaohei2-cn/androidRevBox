import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowRightLeft, OctagonX, Square } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type PortHolder } from "@/api/device";
import { DeviceBar, InfoChip } from "@/features/adb/BinaryHosting";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * 进程端口管理（ADB 页子标签）：PID ↔ 端口互查 + 终止进程。
 * - 端口 → 进程：grep /proc/net/tcp(6) 十六进制端口 → LISTEN 行 inode →
 *   扫 /proc/[0-9]+/fd 找持有者（pid + comm）；
 * - 进程 → 端口：/proc/<pid>/fd socket inode → /proc/net/tcp(6) 还原 LISTEN 端口；
 * - kill 走二次确认（首次点击武装，3s 内再点执行 kill -9），root 身份与查询一致；
 * - Root 开关沿用托管页 su -c id 探测链路。端口→进程反查需要读别人的
 *   /proc/<pid>/fd，不勾选 Root 时结果只覆盖 shell 可见进程（UI 有提示）。
 */

interface PortHit {
  port: number;
  holders: PortHolder[];
}

interface PidHit {
  pid: number;
  ports: { address: string; port: number; family: string }[];
}

const KILL_ARM_MS = 3000;

export function ProcPorts() {
  const { t } = useI18n();
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [root, setRoot] = useState(false);
  const [probing, setProbing] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const [portInput, setPortInput] = useState("");
  const [pidInput, setPidInput] = useState("");
  const [portHit, setPortHit] = useState<PortHit | null>(null);
  const [pidHit, setPidHit] = useState<PidHit | null>(null);
  const [busyA, setBusyA] = useState(false);
  const [busyB, setBusyB] = useState(false);
  const [armedPid, setArmedPid] = useState<number | null>(null);
  const [killing, setKilling] = useState(false);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const adbReady = !!env?.installed;

  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "procports"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);
  useEffect(() => {
    if (!deviceSerial && online.length > 0) setDeviceSerial(online[0].serial);
  }, [deviceSerial, online]);

  // 设备切换：查询结果与 root 探测状态作废
  useEffect(() => {
    setPortHit(null);
    setPidHit(null);
    setNotice(null);
    setRoot(false);
    setArmedPid(null);
  }, [deviceSerial]);

  // 武装状态超时解除
  useEffect(() => {
    if (armedPid === null) return;
    const timer = window.setTimeout(() => setArmedPid(null), KILL_ARM_MS);
    return () => window.clearTimeout(timer);
  }, [armedPid]);


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

  const queryByPort = async () => {
    if (!deviceSerial) return;
    const port = Number(portInput.trim());
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setNotice(t("adb.proc.badPort"));
      return;
    }
    setBusyA(true);
    setArmedPid(null);
    try {
      const holders = await deviceApi.procByPort(deviceSerial, port, root);
      setPortHit({ port, holders });
    } catch (e) {
      setPortHit(null);
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setBusyA(false);
    }
  };

  const queryByPid = async () => {
    if (!deviceSerial) return;
    const pid = Number(pidInput.trim());
    if (!Number.isInteger(pid) || pid < 1) {
      setNotice(t("adb.proc.badPid"));
      return;
    }
    setBusyB(true);
    setArmedPid(null);
    try {
      const ports = await deviceApi.procPorts(deviceSerial, pid, root);
      setPidHit({ pid, ports });
    } catch (e) {
      setPidHit(null);
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setBusyB(false);
    }
  };

  /** 二次确认杀进程：首次点击武装，再次点击执行 */
  const kill = async (pid: number) => {
    if (!deviceSerial) return;
    if (armedPid !== pid) {
      setArmedPid(pid);
      return;
    }
    setArmedPid(null);
    setKilling(true);
    try {
      await deviceApi.binaryKill(deviceSerial, pid, root);
      setNotice(t("adb.proc.killed", { pid }));
      if (portHit) {
        setPortHit({
          ...portHit,
          holders: portHit.holders.filter((h) => h.pid !== pid),
        });
      }
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setKilling(false);
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
    <div className="flex h-full min-h-0 flex-col gap-3 overflow-auto">
      <p className="shrink-0 text-xs leading-relaxed text-muted-foreground">
        {t("adb.proc.description")}
      </p>

      <DeviceBar
        online={online}
        selected={deviceSerial}
        onSelect={setDeviceSerial}
        onRefresh={() => {
          if (portHit) void queryByPort();
          else if (pidHit) void queryByPid();
        }}
        refreshing={busyA || busyB}
        root={root}
        probing={probing}
        onRootChange={(c) => void toggleRoot(c)}
      />

      {!root && (
        <p className="shrink-0 rounded-md border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-[11px] text-amber-600 dark:text-amber-400">
          {t("adb.proc.rootTip")}
        </p>
      )}

      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
        {/* 端口 → 进程 */}
        <section className="flex min-h-0 flex-col gap-2 rounded-lg border bg-card p-3" aria-label={t("adb.proc.byPort")}>
          <h3 className="flex items-center gap-1.5 text-xs font-semibold">
            <ArrowRightLeft className="h-3.5 w-3.5 text-muted-foreground" />
            {t("adb.proc.byPort")}
          </h3>
          <div className="flex shrink-0 items-center gap-2">
            <input
              aria-label={t("adb.proc.portLabel")}
              className="h-7 w-28 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
              placeholder="8080"
              value={portInput}
              onChange={(e) => setPortInput(e.target.value.replace(/\D/g, ""))}
              onKeyDown={(e) => e.key === "Enter" && !busyA && void queryByPort()}
            />
            <Button
              size="sm"
              variant="outline"
              className="h-7 px-2"
              disabled={!deviceSerial || busyA}
              onClick={() => void queryByPort()}
            >
              {t("adb.proc.query")}
            </Button>
          </div>
          <div className="min-h-0 flex-1 overflow-auto text-xs">
            {portHit === null ? (
              <p className="text-muted-foreground">{t("adb.proc.idle")}</p>
            ) : portHit.holders.length === 0 ? (
              <p className="text-muted-foreground">{t("adb.proc.noHolder", { port: portHit.port })}</p>
            ) : (
              <ul className="divide-y">
                {portHit.holders.map((h) => (
                  <li key={h.pid} className="flex items-center gap-2 py-1.5" data-testid={`proc-holder-${h.pid}`}>
                    <InfoChip
                      label={`pid ${h.pid}`}
                      title={t("adb.binary.copyPid")}
                      testid={`proc-pid-${h.pid}`}
                    />
                    <span className="path-selectable min-w-0 flex-1 break-all font-mono">{h.name}</span>
                    <Button
                      size="sm"
                      variant={armedPid === h.pid ? "destructive" : "outline"}
                      className={cn("h-6 shrink-0 gap-1 px-2", armedPid !== h.pid && "text-destructive")}
                      disabled={killing}
                      onClick={() => void kill(h.pid)}
                    >
                      {armedPid === h.pid ? (
                        <OctagonX className="h-3 w-3" />
                      ) : (
                        <Square className="h-3 w-3" />
                      )}
                      {armedPid === h.pid ? t("adb.proc.confirmKill") : t("adb.proc.kill")}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </section>

        {/* 进程 → 端口 */}
        <section className="flex min-h-0 flex-col gap-2 rounded-lg border bg-card p-3" aria-label={t("adb.proc.byPid")}>
          <h3 className="flex items-center gap-1.5 text-xs font-semibold">
            <ArrowRightLeft className="h-3.5 w-3.5 text-muted-foreground" />
            {t("adb.proc.byPid")}
          </h3>
          <div className="flex shrink-0 items-center gap-2">
            <input
              aria-label={t("adb.proc.pidLabel")}
              className="h-7 w-28 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
              placeholder="30743"
              value={pidInput}
              onChange={(e) => setPidInput(e.target.value.replace(/\D/g, ""))}
              onKeyDown={(e) => e.key === "Enter" && !busyB && void queryByPid()}
            />
            <Button
              size="sm"
              variant="outline"
              className="h-7 px-2"
              disabled={!deviceSerial || busyB}
              onClick={() => void queryByPid()}
            >
              {t("adb.proc.query")}
            </Button>
          </div>
          <div className="min-h-0 flex-1 overflow-auto text-xs">
            {pidHit === null ? (
              <p className="text-muted-foreground">{t("adb.proc.idle")}</p>
            ) : pidHit.ports.length === 0 ? (
              <p className="text-muted-foreground">{t("adb.proc.noListen", { pid: pidHit.pid })}</p>
            ) : (
              <div className="flex flex-wrap gap-1.5 pt-1">
                {pidHit.ports.map((p) => {
                  const text = `${p.address}:${p.port}`;
                  return (
                    <InfoChip
                      key={`${p.family}-${text}`}
                      label={text}
                      title={t("adb.binary.copyPort", { family: p.family })}
                      testid={`proc-port-${p.port}`}
                    />
                  );
                })}
              </div>
            )}
          </div>
        </section>
      </div>

      {notice && (
        <p className="shrink-0 break-all rounded-md border bg-muted/40 px-2 py-1 text-xs text-muted-foreground">
          {notice}
        </p>
      )}
    </div>
  );
}
