import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, CircleAlert, CircleCheck, Loader2, RadioTower } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  deviceApi,
  type DeviceEntry,
  type FridaServerStatus,
  type FridaServerStartResult,
} from "@/api/device";
import { envApi } from "@/api/env";
import { hookApi, type PreflightDto } from "@/api/hook";
import { useAppNav, useActiveTab } from "@/app/nav";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import type { FridaSettings } from "./types";
import { remoteSpec } from "./types";

/**
 * 区域 A · 会话设置（左上 1/9，§3）：设备/连接模式/运行模式/目标应用 + 前置检查链。
 * 「启动」跳脚本区选脚本（B 区逐行启动，携带这里的设置快照）；「停止」由 C 区会话头承担。
 */
export function SettingsPanel({
  settings,
  onChange,
  running,
  onGotoScripts,
  onStop,
}: {
  settings: FridaSettings;
  onChange: (patch: Partial<FridaSettings>) => void;
  running: boolean;
  onGotoScripts: () => void;
  onStop?: () => void;
}) {
  const { t } = useI18n();
  const hookActive = useActiveTab("hook");
  const [notice, setNotice] = useState<string | null>(null);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "frida"],
    queryFn: () => deviceApi.list(true),
    enabled: !!env?.installed,
    refetchInterval: hookActive ? 10_000 : false,
  });
  const online = useMemo(
    () => devices.filter((d: DeviceEntry) => d.state === "device"),
    [devices],
  );
  // 单设备自动选中（§3）；设备下线则清空
  useEffect(() => {
    if (!settings.deviceSerial && online.length > 0) {
      onChange({ deviceSerial: online[0].serial });
    } else if (
      settings.deviceSerial &&
      online.length > 0 &&
      !online.some((d) => d.serial === settings.deviceSerial)
    ) {
      onChange({ deviceSerial: online[0].serial });
    }
  }, [online, settings.deviceSerial, onChange]);

  const remote = remoteSpec(settings);
  const { data: preflight } = useQuery({
    queryKey: ["hook", "preflight", remote ?? "usb"],
    queryFn: () => hookApi.preflight(remote),
    refetchInterval: hookActive ? 15_000 : false,
    staleTime: 5_000,
  });

  const { gotoConfig, gotoAdbSubTab, gotoBinarySubTab } = useAppNav();

  const fillForeground = async () => {
    if (!settings.deviceSerial) return;
    try {
      const fg = await envApi.foreground(settings.deviceSerial);
      if (fg.package) onChange({ target: fg.package });
      else setNotice(fg.error ?? fg.hint ?? t("hook.frida.noForeground"));
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 overflow-y-auto overflow-x-hidden rounded-xl border border-border/70 bg-card shadow-card p-2.5 text-xs">
      <h3 className="shrink-0 font-medium">{t("hook.frida.settings")}</h3>

      {/* 设备 */}
      <Field label={t("hook.frida.device")}>
        {online.length === 0 ? (
          <span className="text-muted-foreground">{t("hook.frida.noDevice")}</span>
        ) : (
          <select
            className="h-6 w-full min-w-0 rounded border border-input bg-transparent px-1 font-mono"
            value={settings.deviceSerial ?? ""}
            onChange={(e) => onChange({ deviceSerial: e.target.value })}
          >
            {online.map((d) => (
              <option key={d.serial} value={d.serial}>
                {d.model || d.serial}
              </option>
            ))}
          </select>
        )}
      </Field>

      {/* 连接模式 */}
      <Field label={t("hook.frida.connMode")}>
        <Segmented
          value={settings.connMode}
          disabled={running}
          options={[
            { v: "usb", label: "USB" },
            { v: "remote", label: t("hook.frida.connRemote") },
          ]}
          onChange={(connMode) => onChange({ connMode })}
        />
      </Field>
      {settings.connMode === "remote" && (
        <Field label={t("hook.frida.port")}>
          <input
            className="h-6 w-24 rounded border border-input bg-transparent px-1 font-mono"
            value={settings.port}
            onChange={(e) => onChange({ port: e.target.value.replace(/\D/g, "").slice(0, 5) })}
          />
        </Field>
      )}

      {/* 运行模式 */}
      <Field label={t("hook.frida.runMode")}>
        <Segmented
          value={settings.runMode}
          disabled={running}
          options={[
            { v: "attach", label: "Attach" },
            { v: "spawn", label: "Spawn" },
          ]}
          onChange={(runMode) => onChange({ runMode })}
        />
      </Field>

      {/* 目标应用 */}
      <Field label={t("hook.frida.target")}>
        <div className="flex min-w-0 gap-1">
          <input
            className="h-6 min-w-0 flex-1 rounded border border-input bg-transparent px-1 font-mono"
            placeholder={settings.runMode === "spawn" ? "com.example.app" : t("hook.frida.targetPh")}
            value={settings.target}
            disabled={running}
            onChange={(e) => onChange({ target: e.target.value })}
          />
          <Button
            size="sm"
            variant="outline"
            className="h-6 shrink-0 px-1.5 text-11px"
            disabled={!settings.deviceSerial || running}
            onClick={() => void fillForeground()}
          >
            {t("hook.frida.fromForeground")}
          </Button>
        </div>
        {/*
          这条提示是必要的，不是装饰：以前 attach 留空点启动会换回一句伪装成程序故障的
          报错，用户由此认定"这个模式不该问我要 pid"。空着是什么意思，得在框下面说清楚。
        */}
        <p className="mt-1 text-10px leading-snug text-muted-foreground">
          {t("hook.frida.targetHint")}
        </p>
      </Field>

      {settings.deviceSerial && <FridaServerControl serial={settings.deviceSerial} t={t} />}

      <PreflightList preflight={preflight} remoteMode={settings.connMode === "remote"} t={t}
        onFix={(kind) => {
          if (kind === "adb" || kind === "python") gotoConfig(kind === "adb" ? "app.adb.path" : "app.python.path");
          else if (kind === "remote") gotoAdbSubTab("forward");
          else if (kind === "runner" || kind === "frida-server") gotoBinarySubTab("hosting");
        }}
      />

      <div className="mt-auto flex shrink-0 gap-2 pt-1">
        <Button size="sm" className="h-7 flex-1 gap-1" onClick={onGotoScripts}>
          <ArrowRight className="h-3 w-3" />
          {t("hook.frida.launch")}
        </Button>
        {running && onStop && (
          <Button size="sm" variant="destructive" className="h-7 shrink-0" onClick={onStop}>
            {t("hook.frida.stop")}
          </Button>
        )}
      </div>
      {notice && (
        <p className="shrink-0 break-all rounded bg-muted/50 px-1.5 py-1 text-11px text-muted-foreground">
          {notice}
        </p>
      )}
    </section>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex shrink-0 flex-col gap-1">
      <span className="text-11px text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function Segmented<T extends string>({
  value,
  options,
  onChange,
  disabled,
}: {
  value: T;
  options: { v: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <div
      className={cn("inline-flex rounded-md bg-muted p-0.5", disabled && "opacity-60")}
      role="radiogroup"
    >
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          role="radio"
          aria-checked={value === o.v}
          disabled={disabled}
          onClick={() => onChange(o.v)}
          className={cn(
            "rounded px-2 py-0.5 text-11px transition-colors",
            value === o.v
              ? "bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

type FixKind = "adb" | "python" | "frida" | "remote" | "runner" | "frida-server";

/**
 * 设备侧 frida-server 控制（AR9.1）。
 *
 * 以前这一栏只能提示用户「去托管二进制页自己点两下」，而远程模式的失败原因八成就是
 * 服务没起或不是 root 起的。这里把三件事接进工作台：状态、以 root 启动、停止。
 * 状态文案刻意区分 `running_as_shell`（连得上但注入不了别人）与 `indeterminate`
 * （读不到 uid，不猜），因为这两件事的下一步动作完全不同。
 */
export function FridaServerControl({
  serial,
  t,
}: {
  serial: string;
  t: (key: string, vars?: Record<string, string | number>) => string;
}) {
  const [busy, setBusy] = useState<"start" | "stop" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { data, isError, error, refetch } = useQuery({
    queryKey: ["frida", "server", serial],
    queryFn: () => deviceApi.fridaServerStatus(serial),
    refetchInterval: 15_000,
  });

  const start = async () => {
    setBusy("start");
    setNotice(null);
    try {
      const result: FridaServerStartResult = await deviceApi.fridaServerStart(serial);
      if (result.outcome === "no_op") {
        setNotice(t("hook.frida.server.alreadyRunning", { pid: result.pid ?? "-" }));
      } else if (!result.verified) {
        setNotice(t("hook.frida.server.notVerified"));
      }
      await refetch();
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setBusy(null);
    }
  };

  const stop = async () => {
    setBusy("stop");
    setNotice(null);
    try {
      await deviceApi.fridaServerStop(serial);
      await refetch();
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setBusy(null);
    }
  };

  const describe = (status: FridaServerStatus): { text: string; tone: string } => {
    switch (status.state) {
      case "running_as_root":
        return {
          text: t("hook.frida.server.asRoot", {
            pid: status.pid ?? "-",
            address: `${status.listen_address ?? "127.0.0.1"}:${status.port ?? "-"}`,
            version: status.version ? ` · frida ${status.version}` : "",
          }),
          tone: status.listening ? "text-emerald-500" : "text-amber-500",
        };
      case "running_as_shell":
        return {
          text: t("hook.frida.server.asShell", { uid: status.uid ?? "-" }),
          tone: "text-destructive",
        };
      case "indeterminate":
        return { text: t("hook.frida.server.indeterminate"), tone: "text-amber-500" };
      default:
        return { text: t("hook.frida.server.notRunning"), tone: "text-muted-foreground" };
    }
  };

  const shown: { text: string; tone: string } = isError
    ? { text: String((error as Error)?.message ?? error), tone: "text-destructive" }
    : data
      ? describe(data)
      : { text: t("common.loading"), tone: "text-muted-foreground" };

  return (
    <div className="shrink-0 space-y-0.5 rounded bg-muted/30 p-1.5 text-11px">
      <div className="flex items-center gap-1">
        <RadioTower className="h-3 w-3 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate font-medium">{t("hook.frida.server.title")}</span>
        <Button
          size="sm"
          variant="outline"
          className="h-5 shrink-0 px-1.5 text-10px"
          disabled={busy !== null || !data?.running}
          onClick={() => void stop()}
        >
          {t("hook.frida.server.stop")}
        </Button>
        <Button
          size="sm"
          className="h-5 shrink-0 px-1.5 text-10px"
          disabled={busy !== null || (data?.running && data?.as_root && data?.listening)}
          onClick={() => void start()}
        >
          {busy === "start" ? t("hook.frida.server.starting") : t("hook.frida.server.start")}
        </Button>
      </div>
      <div className={cn("truncate", shown.tone)} title={shown.text}>
        {shown.text}
      </div>
      {notice && <div className="break-all text-muted-foreground">{notice}</div>}
    </div>
  );
}

function PreflightList({
  preflight,
  remoteMode,
  t,
  onFix,
}: {
  preflight: PreflightDto | undefined;
  remoteMode: boolean;
  t: (k: string) => string;
  onFix: (kind: FixKind) => void;
}) {
  if (!preflight) {
    return (
      <div className="flex shrink-0 items-center gap-1 text-11px text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" /> {t("hook.frida.preflightChecking")}
      </div>
    );
  }
  const items: { ok: boolean; label: string; hint?: string | null; fix?: FixKind }[] = [
    { ok: preflight.adbOk, label: "ADB", hint: preflight.adbHint, fix: "adb" },
    { ok: preflight.pythonOk, label: "Python", hint: preflight.pythonHint, fix: "python" },
    { ok: preflight.fridaOk, label: `frida${preflight.fridaVersion ? " " + preflight.fridaVersion : ""}`, hint: preflight.fridaHint, fix: "frida" },
    { ok: preflight.runnerOk, label: t("hook.frida.runner"), hint: preflight.runnerHint, fix: "runner" },
  ];
  if (remoteMode) {
    items.push({
      ok: preflight.remoteOk === true,
      label: t("hook.frida.remoteReachable"),
      hint: preflight.remoteHint,
      fix: "remote",
    });
  }
  const allOk = items.every((i) => i.ok);
  return (
    <div className="shrink-0 space-y-0.5 rounded bg-muted/30 p-1.5">
      <div className="flex items-center gap-1 text-11px font-medium">
        <CircleCheck className={cn("h-3 w-3", allOk ? "text-emerald-500" : "text-muted-foreground")} />
        {t("hook.frida.preflight")}
      </div>
      {items.map((i) => (
        <div key={i.label} className="flex items-center gap-1 text-11px">
          {i.ok ? (
            <CircleCheck className="h-3 w-3 shrink-0 text-emerald-500" />
          ) : (
            <CircleAlert className="h-3 w-3 shrink-0 text-destructive" />
          )}
          <span className={cn("min-w-0 flex-1 truncate", !i.ok && "text-destructive")} title={i.hint ?? i.label}>
            {i.label}
          </span>
          {!i.ok && i.fix && i.fix !== "frida" && (
            <button
              type="button"
              className={cn(
                "shrink-0 inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-10px",
                i.fix === "remote" ? "bg-destructive/10 text-destructive hover:bg-destructive/20" : "bg-muted hover:bg-muted/70",
              )}
              onClick={() => onFix(i.fix!)}
              title={i.hint ?? ""}
            >
              {i.fix === "remote" && <RadioTower className="h-2.5 w-2.5" />}
              {i.fix === "remote" ? t("hook.frida.gotoForward") : t("hook.frida.fix")}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
