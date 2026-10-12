import { useEffect, useMemo, useState } from "react";
import { Select } from "@/components/ui/select";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowUpRight,
  CircleAlert,
  CircleCheck,
  CircleStop,
  Loader2,
  RadioTower,
  Settings2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { PaneHeader } from "@/components/ui/pane-header";
import {
  deviceApi,
  type DeviceEntry,
  type FridaServerStatus,
} from "@/api/device";
import { envApi } from "@/api/env";
import { hookApi, type PreflightDto } from "@/api/hook";
import { useAppNav, useActiveTab } from "@/app/nav";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import type { FridaSettings } from "./types";
import { remoteSpec } from "./types";

/**
 * 区域 A · 会话设置：设备 / 连接模式（+远程端口）/ 运行模式 / 目标应用，
 * 外加 frida-server 状态与前置检查药丸。四件控件两行放完，面板高度由内容决定（不出滚动条）。
 *
 * 这里**没有**「启动」按钮：启动是 B 区每一行的动作（▶ 或编辑器里的「运行」，
 * 都带上本区的设置快照）。这里**也没有**「停止」：会话跑在右侧终端页，
 * 停止就是那块屏自己标签页上的关闭（结构化控制台退役后，跨视图的"唯一停止入口"
 * 已无存在必要——终端页随时都在，且每条会话各停各的）。
 */
export function SettingsPanel({
  settings,
  onChange,
}: {
  settings: FridaSettings;
  onChange: (patch: Partial<FridaSettings>) => void;
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
    // 换设备必须重新探：不然绿的是上一台机的 frida-server
    queryKey: ["hook", "preflight", remote ?? "usb", settings.deviceSerial ?? "none"],
    queryFn: () =>
      hookApi.preflight(remote, settings.connMode === "usb" ? (settings.deviceSerial ?? undefined) : undefined),
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

  const [stopping, setStopping] = useState(false);
  const targetPkg = settings.target.trim();
  // attach 恒等于"附加设备当前前台"（见 fridaSession.ts 的 targetArgs），所以输入框与
  // 「取前台」在这一模式下禁用：一个填了反而起不来的框，不如直接不给填。
  const attachMode = settings.runMode === "attach";
  // 强停的目标：attach 现取前台包名；spawn 用框里填的包名（纯数字不算包名，不给点）。
  const canForceStop = Boolean(
    settings.deviceSerial && (attachMode || (targetPkg && !/^\d+$/.test(targetPkg))),
  );

  /** 与应用长按菜单里的「强行停止」同一条路：device_force_stop，带设备侧复核 */
  const forceStopTarget = async () => {
    const serial = settings.deviceSerial;
    if (!serial || !canForceStop) return;
    setStopping(true);
    setNotice(null);
    try {
      let pkg = targetPkg;
      if (attachMode) {
        // attach 下没有"填好的包名"可用，强停的就是那只前台 —— 现场向设备取一次。
        const fg = await envApi.foreground(serial);
        pkg = fg.package?.trim() ?? "";
        if (!pkg) {
          setNotice(t("hook.frida.forceStopNoForeground", { hint: fg.error ?? fg.hint ?? "-" }));
          return;
        }
      }
      const r = await deviceApi.forceStop(serial, pkg);
      if (r.outcome === "replayed") {
        setNotice(t("hook.frida.forceStopReplayed", { pkg }));
      } else if (r.outcome === "no_op") {
        setNotice(t("hook.frida.forceStopNoOp", { pkg }));
      } else if (!r.verified) {
        setNotice(t("hook.frida.forceStopUnverified", { detail: r.detail ?? "-" }));
      } else {
        setNotice(t("hook.frida.forceStopDone", { pkg }));
      }
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setStopping(false);
    }
  };

  return (
    // 不设 h-full、不开 overflow-y-auto：A 区的高度由内容决定（见 FridaPage 的
    // grid-rows-[auto_minmax(0,1fr)]），"会话设置"因此永远整块可见；
    // 剩余高度全给 B 区脚本列表，要滚也是它滚。
    <section className="flex shrink-0 flex-col gap-1.5 rounded-xl border border-border/70 bg-card shadow-card p-2 text-xs">
      <PaneHeader compact icon={Settings2} title={t("hook.frida.settings")} />

      {/*
        四个控件压进两行。原来一件一行、标签还单独占一行，光这四件就吃掉 ~150px，
        加上前置检查就是滚动条——会话设置要一眼看全，不能靠滚。
      */}
      <div className="flex shrink-0 items-center gap-1.5">
        <RowLabel>{t("hook.frida.device")}</RowLabel>
        {online.length === 0 ? (
          <span className="min-w-0 flex-1 truncate text-11px text-muted-foreground">
            {t("hook.frida.noDevice")}
          </span>
        ) : (
          <Select
            containerClassName="min-w-0 flex-1"
            className="h-6 w-full font-mono"
            data-testid="frida-device"
            value={settings.deviceSerial ?? ""}
            onChange={(e) => onChange({ deviceSerial: e.target.value })}
          >
            {online.map((d) => (
              <option key={d.serial} value={d.serial}>
                {d.model || d.serial}
              </option>
            ))}
          </Select>
        )}
        <RowLabel>{t("hook.frida.connMode")}</RowLabel>
        <Segmented
          value={settings.connMode}
          options={[
            { v: "usb", label: "USB" },
            { v: "remote", label: t("hook.frida.connRemote") },
          ]}
          onChange={(connMode) => onChange({ connMode })}
        />
      </div>

      {/*
        第 2 行是"连接目标"：它**永远在**，切换 USB / 远程只换这一行的内容。
        以前端口输入框塞在第 1 行里，一点"远程端口"整行的东西就横向跳一格——
        布局被状态改变推着走，手会去点错地方。现在改成"加一行展示变化"。
      */}
      <div className="flex shrink-0 items-center gap-1.5" data-testid="frida-endpoint-row">
        <RowLabel>{t("hook.frida.endpoint")}</RowLabel>
        <code
          className="min-w-0 flex-1 truncate font-mono text-11px text-muted-foreground"
          title={remote ? `tcp://${remote}` : `usb:${settings.deviceSerial ?? "-"}`}
        >
          {remote ? remote : `usb · ${settings.deviceSerial ?? "-"}`}
        </code>
        {settings.connMode === "remote" && (
          <>
            <RowLabel>{t("hook.frida.port")}</RowLabel>
            <input
              aria-label={t("hook.frida.port")}
              data-testid="frida-port"
              inputMode="numeric"
              className="h-6 w-16 shrink-0 rounded-md border border-input bg-background/70 px-1.5 font-mono text-11px focus-visible:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
              value={settings.port}
                  onChange={(e) => onChange({ port: e.target.value.replace(/\D/g, "").slice(0, 5) })}
            />
          </>
        )}
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
        <RowLabel>{t("hook.frida.runMode")}</RowLabel>
        <Segmented
          value={settings.runMode}
          options={[
            { v: "attach", label: "Attach" },
            { v: "spawn", label: "Spawn" },
          ]}
          onChange={(runMode) => onChange({ runMode })}
        />
        {/* 目标应用不给可见标签：placeholder 已随模式变化，名字走 aria-label。
            attach 模式下这个框禁用并且不显示残留值 —— 那个模式的目标由设备回答。 */}
        <input
          aria-label={t("hook.frida.target")}
          data-testid="frida-target"
          className="h-6 min-w-28 flex-1 rounded-md border border-input bg-background/70 px-1.5 font-mono text-11px focus-visible:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          placeholder={attachMode ? t("hook.frida.attachFrontmost") : "com.example.app"}
          title={attachMode ? t("hook.frida.attachFrontmost") : t("hook.frida.targetHint")}
          value={attachMode ? "" : settings.target}
          disabled={attachMode}
          onChange={(e) => onChange({ target: e.target.value })}
        />
        <Button
          size="sm"
          variant="outline"
          className="h-6 shrink-0 px-1.5 text-11px"
          disabled={!settings.deviceSerial || attachMode}
          title={attachMode ? t("hook.frida.attachFrontmost") : undefined}
          onClick={() => void fillForeground()}
        >
          {t("hook.frida.fromForeground")}
        </Button>
        {/*
          强行停止 = 系统「应用信息 → 强行停止」那一下（am force-stop），
          杀的是目标应用的**全部进程**，frida 会话随之结束，所以它是危险动作：
          只给图标、描边色，不占主色；目标为空或填的是 pid 时不给点（没有可强停的包名）。
        */}
        <Button
          size="icon"
          variant="outline"
          className="h-6 w-6 shrink-0 text-muted-foreground hover:border-destructive/50 hover:text-destructive"
          aria-label={t("hook.frida.forceStop")}
          title={canForceStop ? t("hook.frida.forceStopHint") : t("hook.frida.forceStopNeedPkg")}
          data-testid="frida-force-stop"
          disabled={!canForceStop || stopping}
          onClick={() => void forceStopTarget()}
        >
          {stopping ? <Loader2 className="h-3 w-3 animate-spin" /> : <CircleStop className="h-3 w-3" />}
        </Button>
      </div>
      {settings.deviceSerial && (
        <FridaServerControl
          serial={settings.deviceSerial}
          t={t}
          // 握手结果与二进制叫什么**无关**，所以拿它兜住"改了名的 server"：
          // 通道通了就不能说未运行。
          channel={{ ok: preflight?.channelOk ?? null, hint: preflight?.channelHint ?? null }}
          port={settings.port.trim() || "27042"}
        />
      )}

      <PreflightList preflight={preflight} remoteMode={settings.connMode === "remote"} t={t}
        onFix={(kind) => {
          if (kind === "adb" || kind === "python") gotoConfig(kind === "adb" ? "app.adb.path" : "app.python.path");
          else if (kind === "remote") gotoAdbSubTab("forward");
          else if (kind === "frida-server") gotoBinarySubTab("hosting");
        }}
      />

      {notice && (
        <p className="shrink-0 break-all rounded bg-muted/50 px-1.5 py-1 text-11px text-muted-foreground">
          {notice}
        </p>
      )}
    </section>
  );
}

/** 行内小标签：标签单独占一行是这次滚动条的主因之一 */
function RowLabel({ children }: { children: React.ReactNode }) {
  return <span className="shrink-0 text-11px text-muted-foreground">{children}</span>;
}

export function Segmented<T extends string>({
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
              ? // 选中态走主色（和设置页分段选择、左侧 tab 齿块、启动按钮同一个蓝）
                "bg-primary text-primary-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

type FixKind = "adb" | "python" | "frida" | "remote" | "frida-server";

/**
 * 设备侧 frida-server 状态（AR9.1）：**只读状态 + 一个跳转**。
 *
 * 启停不在这儿做。以前这里塞了「以 root 启动」「停止」两个按钮，看着是省事，
 * 实际是给同一件事开了第二个入口，而这个面板根本不知道起的是哪个文件、什么版本、
 * 日志在哪——那些都在「二进制」页的托管列表里。现在状态留在这里（它是配置决策要看的），
 * 动作变成一个箭头跳过去。
 *
 * 状态文案刻意区分 `running_as_shell`（连得上但注入不了别人）与 `indeterminate`
 * （读不到 uid，不猜）：这两件事的下一步动作完全不同。
 */
export function FridaServerControl({
  serial,
  t,
  channel,
  port,
}: {
  serial: string;
  t: (key: string, vars?: Record<string, string | number>) => string;
  /** 宿主侧 frida 握手结论：null = 还没探过（Python 没就绪时就是 null，不猜） */
  channel?: { ok: boolean | null; hint?: string | null };
  port?: string;
}) {
  const { gotoBinarySubTab } = useAppNav();
  const { data, isError, error } = useQuery({
    queryKey: ["frida", "server", serial],
    queryFn: () => deviceApi.fridaServerStatus(serial),
    refetchInterval: 15_000,
  });

  const describe = (status: FridaServerStatus): { text: string; tone: string } => {
    switch (status.state) {
      case "running_as_root":
        return {
          text: t("hook.frida.server.asRoot", {
            pid: status.pid ?? "-",
            address: `${status.listen_address ?? "127.0.0.1"}:${status.port ?? "-"}`,
            version: status.version ? ` · frida ${status.version}` : "",
          }),
          tone: status.listening ? "text-emerald-600 dark:text-emerald-400" : "text-amber-600 dark:text-amber-400",
        };
      case "running_as_shell":
        return {
          text: t("hook.frida.server.asShell", { uid: status.uid ?? "-" }),
          tone: "text-destructive",
        };
      case "indeterminate":
        return { text: t("hook.frida.server.indeterminate"), tone: "text-amber-600 dark:text-amber-400" };
      default:
        return { text: t("hook.frida.server.notRunning"), tone: "text-muted-foreground" };
    }
  };

  const shown: { text: string; tone: string } = (() => {
    if (isError) return { text: String((error as Error)?.message ?? error), tone: "text-destructive" };
    if (data?.running) return describe(data);
    // Agent 按进程名 `frida-server` 找人。改名过的 server（魔改常干这事）它认不出，
    // 但 frida 握手是**与名字无关**的证据：通道通了还说"未运行"就是骗人。
    if (channel?.ok === true) {
      return {
        text: t("hook.frida.server.throughUnnamed", { port: port ?? "27042" }),
        tone: "text-emerald-600 dark:text-emerald-400",
      };
    }
    if (channel?.ok === false) {
      return { text: channel.hint ?? t("hook.frida.server.notRunning"), tone: "text-destructive" };
    }
    if (!data) return { text: t("common.loading"), tone: "text-muted-foreground" };
    return describe(data);
  })();

  return (
    <div className="flex shrink-0 items-center gap-1 text-11px">
      <span
        className={cn(
          "inline-flex h-5 min-w-0 flex-1 items-center gap-1 rounded-full bg-muted px-1.5",
          shown.tone,
        )}
      >
        <RadioTower className="h-2.5 w-2.5 shrink-0 text-muted-foreground" />
        <span className="shrink-0 font-medium text-muted-foreground">{t("hook.frida.server.title")}</span>
        {/* 长文案截断，全文在 title 里：这一行不许因为版本信息变高 */}
        <span className="min-w-0 flex-1 truncate" title={shown.text}>
          {shown.text}
        </span>
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-5 w-5 shrink-0 rounded-md text-muted-foreground hover:bg-primary/10 hover:text-primary"
        onClick={() => gotoBinarySubTab("hosting")}
        aria-label={t("hook.frida.server.gotoHosting")}
        title={t("hook.frida.server.gotoHosting")}
        data-testid="frida-server-goto-hosting"
      >
        <ArrowUpRight className="h-3 w-3" />
      </Button>
    </div>
  );
}

/** 导出给测试：三态（没探 / 探失败 / 探通）的渲染差异是这里最容易写错的地方 */
export function PreflightList({
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
    {
      // 这一项判的是官方 REPL 要的 **frida-tools**；显示优先用 tools 版本
      // （它才是被 REPL 直接用到的那一版），没有就退回绑定包版本
      ok: preflight.fridaOk,
      label: `frida${
        (preflight.fridaToolsVersion ?? preflight.fridaVersion)
          ? " " + (preflight.fridaToolsVersion ?? preflight.fridaVersion)
          : ""
      }`,
      hint: preflight.fridaHint,
      fix: "frida",
    },
  ];
  // 握手探活只在"探过"的时候出现：没 Python 时画一条红字只是噪音，画绿勾又是撒谎
  if (preflight.channelOk !== null && preflight.channelOk !== undefined) {
    items.push({
      ok: preflight.channelOk === true,
      label: t("hook.frida.channel"),
      hint: preflight.channelHint,
      fix: "frida-server",
    });
  }
  if (remoteMode) {
    items.push({
      ok: preflight.remoteOk === true,
      label: t("hook.frida.remoteReachable"),
      hint: preflight.remoteHint,
      fix: "remote",
    });
  }
  // 一行药丸：过的绿、不过的红；失败且知道去哪修的，直接在药丸上给动作。
  // 原来是一个带标题的列表块（每项一行、最多 6 行），A 区放不下的一半原因就在这。
  // 刻意不做"汇总绿勾"：单项有绿有红时，一个总体图标不是撒谎就是噪音。
  return (
    <div
      role="group"
      aria-label={t("hook.frida.preflight")}
      className="flex shrink-0 flex-wrap items-center gap-1"
    >
      {items.map((i) => {
        const canFix = !i.ok && Boolean(i.fix) && i.fix !== "frida";
        const Icon = i.ok ? CircleCheck : CircleAlert;
        return (
          <button
            key={i.label}
            type="button"
            disabled={i.ok || !canFix}
            onClick={() => canFix && onFix(i.fix!)}
            title={i.hint ?? i.label}
            className={cn(
              "inline-flex h-5 max-w-full shrink-0 items-center gap-1 rounded-full px-1.5 text-10px transition-colors",
              i.ok
                ? "bg-emerald-500/12 text-emerald-600 dark:text-emerald-400"
                : "bg-destructive/12 text-destructive",
              canFix && "hover:bg-destructive/25",
            )}
          >
            <Icon
              className={cn(
                "h-2.5 w-2.5 shrink-0",
                i.ok ? "text-emerald-600 dark:text-emerald-400" : "text-destructive",
              )
            } />
            <span className="min-w-0 max-w-40 truncate">{i.label}</span>
            {canFix && (
              <span className="shrink-0 font-medium opacity-80">
                {i.fix === "remote" ? t("hook.frida.gotoForward") : t("hook.frida.fix")}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
