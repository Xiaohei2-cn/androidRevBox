import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, CircleSlash, Plus, RefreshCw, XCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AdbNotReadyState } from "@/components/ui/adb-gate";
import { deviceApi, type DeviceEntry, type ForwardRule } from "@/api/device";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * 端口转发管理（ADB 页子标签）：
 * - 默认五行，不可删减；需要更多时可增行（增行可删）；
 * - 本地地址默认 127.0.0.1 灰色锁定；勾选「自定义」后可编辑；
 * - 勾选「用设备 IP」把本地地址换成所选设备的 wlan0 IP（多台在线弹清单选择）；
 * - 每行「验证」走 adb -s <serial> forward --list 比对；「全部验证」批量刷新。
 * 所有 adb 调用后端均 -s 绑定设备，多设备互不串扰。
 */

interface RowState {
  id: number;
  /** 本地地址：默认 127.0.0.1；custom 开启后可编辑 */
  host: string;
  /** 勾选「自定义」= 允许编辑 host */
  custom: boolean;
  /** 勾选「用设备 IP」= host 被设备 IP 覆盖 */
  useDeviceIp: boolean;
  localPort: string;
  remote: string;
}

type RowVerify =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "active"; connected: boolean }
  | { kind: "missing" }
  | { kind: "invalid" };

let nextRowId = 6;

const DEFAULT_ROWS: RowState[] = [1, 2, 3, 4, 5].map((n) => ({
  id: n,
  host: "127.0.0.1",
  custom: false,
  useDeviceIp: false,
  localPort: "",
  remote: "",
}));

export function ForwardManager() {
  const { t } = useI18n();
  const [rows, setRows] = useState<RowState[]>(DEFAULT_ROWS);
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [verify, setVerify] = useState<Record<number, RowVerify>>({});
  const [notice, setNotice] = useState<string | null>(null);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const adbReady = !!env?.installed;

  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "forward"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);

  // 默认跟随第一台在线设备（多台时保持当前选择，不动）
  useEffect(() => {
    if (!deviceSerial && online.length > 0) setDeviceSerial(online[0].serial);
  }, [deviceSerial, online]);

  const patch = useCallback((id: number, p: Partial<RowState>) => {
    setRows((rs) => rs.map((r) => (r.id === id ? { ...r, ...p } : r)));
  }, []);

  const addRow = () => {
    setRows((rs) => [
      ...rs,
      { id: nextRowId++, host: "127.0.0.1", custom: false, useDeviceIp: false, localPort: "", remote: "" },
    ]);
  };

  const setVerifyState = (id: number, v: RowVerify) =>
    setVerify((s) => ({ ...s, [id]: v }));

  if (!adbReady) {
    return <AdbNotReadyState hint={env?.hint} />;
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <p className="text-xs leading-relaxed text-muted-foreground">{t("adb.forward.description")}</p>

      <DevicePicker online={online} selected={deviceSerial} onSelect={setDeviceSerial} />

      <div className="min-h-0 flex-1 space-y-2 overflow-auto pr-1">
        {rows.map((row, idx) => (
          <ForwardRow
            key={row.id}
            row={row}
            index={idx + 1}
            serial={deviceSerial}
            removable={rows.length > DEFAULT_ROWS.length}
            verify={verify[row.id] ?? { kind: "idle" }}
            online={online}
            onPatch={(p) => patch(row.id, p)}
            onRemove={() => setRows((rs) => rs.filter((r) => r.id !== row.id))}
            onNotice={setNotice}
            onVerifyState={(v) => setVerifyState(row.id, v)}
          />
        ))}
      </div>

      <div className="flex shrink-0 items-center gap-2">
        <Button size="sm" variant="outline" onClick={addRow}>
          <Plus className="h-3.5 w-3.5" />
          {t("adb.forward.addRow")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!deviceSerial}
          onClick={async () => {
            if (!deviceSerial) return;
            try {
              const rules = await deviceApi.forwardList(deviceSerial);
              if (rules.length === 0) {
                setNotice(t("adb.forward.noRules"));
                return;
              }
              setNotice(
                rules
                  .map((r) => `${r.local} → ${r.remote}`)
                  .join("  |  "),
              );
            } catch (e) {
              setNotice(String((e as Error)?.message ?? e));
            }
          }}
        >
          <RefreshCw className="h-3.5 w-3.5" />
          {t("adb.forward.currentRules")}
        </Button>
      </div>

      {notice && (
        <p className="shrink-0 break-all rounded-md border bg-muted/40 px-2 py-1 font-mono text-xs text-muted-foreground">
          {notice}
        </p>
      )}
    </div>
  );
}

function DevicePicker({
  online,
  selected,
  onSelect,
}: {
  online: DeviceEntry[];
  selected: string | null;
  onSelect: (s: string) => void;
}) {
  const { t } = useI18n();
  if (online.length === 0) {
    return <p className="text-xs text-muted-foreground">{t("adb.forward.rowInactive")}</p>;
  }
  if (online.length === 1) {
    return (
      <p className="font-mono text-xs text-muted-foreground">
        -s {online[0].serial}
      </p>
    );
  }
  return (
    <label className="flex items-center gap-2 text-xs">
      <span className="text-muted-foreground">-s</span>
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
  );
}

function ForwardRow({
  row,
  index,
  serial,
  removable,
  verify,
  online,
  onPatch,
  onRemove,
  onNotice,
  onVerifyState,
}: {
  row: RowState;
  index: number;
  serial: string | null;
  removable: boolean;
  verify: RowVerify;
  online: DeviceEntry[];
  onPatch: (p: Partial<RowState>) => void;
  onRemove: () => void;
  onNotice: (msg: string) => void;
  onVerifyState: (v: RowVerify) => void;
}) {
  const { t } = useI18n();
  const hostEditable = row.custom && !row.useDeviceIp;
  const configured = !!serial && row.localPort.trim() !== "" && row.remote.trim() !== "";

  const effectiveHost = row.host;
  const localSpec = `tcp:${row.localPort.trim()}`;
  /** 前端归一：设备侧裸数字（用户只填端口）自动按 tcp: 处理，与后端兜底一致 */
  const remoteSpec = /^\d+$/.test(row.remote.trim())
    ? `tcp:${row.remote.trim()}`
    : row.remote.trim();

  /** 勾选「用设备 IP」：单设备直接填；多设备弹清单让用户选 */
  const handleUseDeviceIp = async (checked: boolean) => {
    if (!checked) {
      onPatch({ useDeviceIp: false, host: row.custom ? row.host : "127.0.0.1" });
      return;
    }
    let target: DeviceEntry | undefined;
    if (online.length === 1) {
      target = online[0];
    } else if (online.length > 1) {
      const currentIdx = online.findIndex((d) => d.serial === serial);
      const picked = window.prompt(
        `${t("adb.forward.selectDevice")}\n${online.map((d, i) => `${i + 1}. ${d.model || d.serial}（${d.serial}）`).join("\n")}`,
        String(currentIdx >= 0 ? currentIdx + 1 : 1),
      );
      const idx = (picked ? Number(picked) : NaN) - 1;
      target = Number.isInteger(idx) && idx >= 0 && idx < online.length ? online[idx] : undefined;
      if (!target) return;
    } else {
      onNotice(t("adb.forward.rowInactive"));
      return;
    }
    try {
      const ip = await deviceApi.ip(target.serial);
      if (!ip) {
        onNotice(t("adb.forward.rowInactive"));
        return;
      }
      onPatch({ useDeviceIp: true, host: ip });
    } catch (e) {
      onNotice(String((e as Error)?.message ?? e));
    }
  };

  const apply = async () => {
    if (!serial) return;
    if (!row.localPort.trim()) {
      onNotice(t("adb.forward.needLocalPort"));
      return;
    }
    if (!row.remote.trim()) {
      onNotice(t("adb.forward.needRemote"));
      return;
    }
    try {
      const rule = await deviceApi.forwardSetup(serial, localSpec, remoteSpec);
      onNotice(t("adb.forward.applyOk", { local: rule.local, remote: rule.remote }));
      await doVerify();
    } catch (e) {
      onNotice(String((e as Error)?.message ?? e));
    }
  };

  const doVerify = async () => {
    if (!serial) return;
    if (!row.localPort.trim() || !row.remote.trim()) {
      onVerifyState({ kind: "invalid" });
      return;
    }
    onVerifyState({ kind: "checking" });
    try {
      const rules = await deviceApi.forwardList(serial);
      const hit = rules.find((r: ForwardRule) => r.local === localSpec);
      if (hit && hit.remote === remoteSpec) {
        onVerifyState({ kind: "active", connected: false });
      } else if (hit) {
        // 本地端口被别的远端占用：视作未按本行配置生效
        onVerifyState({ kind: "missing" });
      } else {
        onVerifyState({ kind: "missing" });
      }
    } catch (e) {
      onNotice(String((e as Error)?.message ?? e));
      onVerifyState({ kind: "idle" });
    }
  };


  return (
    <div
      data-testid={`forward-row-${index}`}
      className={cn(
        "grid grid-cols-[1fr_auto] items-center gap-2 rounded-xl border border-border/70 bg-card shadow-card p-2",
        !configured && "opacity-80",
      )}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
        <span className="w-8 shrink-0 text-muted-foreground">{index}</span>
        <input
          type="checkbox"
          aria-label={t("adb.forward.customIp")}
          checked={row.custom}
          onChange={(e) =>
            onPatch({ custom: e.target.checked, host: e.target.checked ? row.host : "127.0.0.1" })
          }
        />
        <input
          className="h-7 w-28 rounded-md border border-input bg-transparent px-2 font-mono text-xs disabled:text-muted-foreground/60"
          value={effectiveHost}
          disabled={!hostEditable}
          onChange={(e) => onPatch({ host: e.target.value })}
        />
        <span className="text-muted-foreground">:</span>
        <input
          className="h-7 w-20 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
          placeholder="8080"
          value={row.localPort}
          onChange={(e) => onPatch({ localPort: e.target.value.replace(/\D/g, "") })}
        />
        <span className="text-muted-foreground">→</span>
        <input
          className="h-7 w-36 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
          placeholder="tcp:8080"
          value={row.remote}
          onChange={(e) => onPatch({ remote: e.target.value })}
        />
        <label className="flex shrink-0 items-center gap-1 text-muted-foreground">
          <input
            type="checkbox"
            aria-label={t("adb.forward.useDeviceIp")}
            checked={row.useDeviceIp}
            onChange={(e) => void handleUseDeviceIp(e.target.checked)}
          />
          {t("adb.forward.useDeviceIp")}
        </label>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <VerifyMark state={verify} />
        <Button size="sm" variant="outline" className="h-7 px-2" disabled={!configured} onClick={() => void apply()}>
          {t("adb.forward.apply")}
        </Button>
        <Button size="sm" variant="outline" className="h-7 px-2" disabled={!configured} onClick={() => void doVerify()}>
          {t("adb.forward.verify")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-7 px-2 text-destructive"
          disabled={!removable}
          title={removable ? t("adb.forward.removeRow") : undefined}
          onClick={() => {
            onRemove();
            if (serial && row.localPort.trim()) {
              void deviceApi
                .forwardRemove(serial, `tcp:${row.localPort.trim()}`)
                .catch(() => undefined);
            }
          }}
        >
          {t("adb.forward.remove")}
        </Button>
      </div>
    </div>
  );
}

function VerifyMark({ state }: { state: RowVerify }) {
  const { t } = useI18n();
  if (state.kind === "active") {
    return (
      <span className="flex items-center gap-1 text-10px text-emerald-500" title={t("adb.forward.verifiedInactive")}>
        <CheckCircle2 className="h-3 w-3" />
      </span>
    );
  }
  if (state.kind === "missing") {
    return (
      <span className="flex items-center gap-1 text-10px text-amber-500" title={t("adb.forward.notFound")}>
        <XCircle className="h-3 w-3" />
      </span>
    );
  }
  if (state.kind === "invalid") {
    return (
      <span className="flex items-center gap-1 text-10px text-red-500" title={t("adb.forward.invalid")}>
        <XCircle className="h-3 w-3" />
      </span>
    );
  }
  return (
    <span className="flex items-center text-10px text-muted-foreground" title={t("adb.forward.rowInactive")}>
      <CircleSlash className={cn("h-3 w-3", state.kind === "checking" && "animate-spin")} />
    </span>
  );
}
