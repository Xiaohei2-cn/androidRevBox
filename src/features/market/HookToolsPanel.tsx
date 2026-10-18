import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Copy, ExternalLink, Loader2, PackageCheck, Wrench } from "lucide-react";

import { Button } from "@/components/ui/button";
import { IconTile } from "@/components/ui/icon-tile";
import { PageHeader } from "@/components/ui/page-header";
import { Pill } from "@/components/ui/pill";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n";
import { deviceApi, type DeviceEntry } from "@/api/device";
import { formatBytes, marketApi, type MarketTool, type ToolInstallReport } from "@/api/market";
import { pickTrackedDevice } from "@/features/binary/hostedRecords";

/**
 * 工具市场 · hook 工具（P13 第一个插件）。
 *
 * 布局：左侧是一列**气泡卡片**（不是传统表格行），右侧是这个工具的操作区。
 *
 * 为什么左侧用气泡而不是表格：这里列的不是"同一类东西的不同实例"（进程、端口、文件），
 * 而是**每个都长得不一样**的工具——名字、用途、体积各说各的。表格行会把它们压成
 * 几列等宽的字段，看一列就知道全是"工具"；气泡卡片让每一张自己成句，选中的那张抬起
 * 来，与右侧正在操作的是同一个东西，这个对应关系是布局本身给的，不靠标题重复。
 *
 * 右侧只做三件事：说清它是什么（说明 + 官方地址 + 版本/体积/许可）、给用法示例、
 * 一键安装到设备。安装的每一步都在后端做完（下载 → SHA-256 → push → chmod → 回读），
 * 这里只负责把"装到哪、多大、验没验过"如实摆出来。
 */
export function HookToolsPanel() {
  const { t } = useI18n();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [report, setReport] = useState<ToolInstallReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const { data: tools = [], isLoading } = useQuery({
    queryKey: ["market", "tools"],
    queryFn: () => marketApi.tools(),
    // 目录是代码里的常量：不轮询、不重取，除非页面重开
    staleTime: Infinity,
  });

  const { data: devices = [], status: devicesStatus } = useQuery({
    queryKey: ["devices", "market"],
    queryFn: () => deviceApi.list(true),
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);

  // 与二进制托管同一套跟设备规则：adb 明确报了别的设备在线才改选，
  // 一瞬间枚举到空列表（换线 / adb 重启）不该把用户正在用的设备甩掉。
  useEffect(() => {
    if (devicesStatus !== "success") return;
    const candidates = [...online.map((d) => d.serial), ...devices.map((d) => d.serial)];
    const next = pickTrackedDevice(deviceSerial, candidates);
    if (next.serial !== deviceSerial) setDeviceSerial(next.serial);
  }, [deviceSerial, devices, devicesStatus, online]);

  // 目录到手后默认选第一个：右侧空着的时候用户还得先猜"点哪里"
  useEffect(() => {
    if (selectedId === null && tools.length > 0) setSelectedId(tools[0].id);
  }, [selectedId, tools]);

  const selected = tools.find((tool) => tool.id === selectedId) ?? null;

  // 设备 ABI 单独问：`devices_list` 里没有这一项，而"能不能跑"只能由设备自己回答。
  // 问不到就是未知（不拦安装），只有明确拿到且不在支持列表里才拦。
  const { data: abi = null } = useQuery({
    queryKey: ["market", "abi", deviceSerial],
    queryFn: () => marketApi.deviceAbi(deviceSerial as string),
    enabled: !!deviceSerial,
    staleTime: 60_000,
  });
  // 设备 ABI 拿不到时不做判断（未知 ≠ 不兼容）：只有明确拿到了且不在支持列表里才拦
  const abiMismatch =
    !!selected && !!abi && selected.deviceAbi.length > 0 && !selected.deviceAbi.includes(abi);

  const install = async (tool: MarketTool) => {
    if (!deviceSerial) return;
    setBusyId(tool.id);
    setError(null);
    setReport(null);
    try {
      const result = await marketApi.install(deviceSerial, tool.id);
      setReport(result);
    } catch (e) {
      setError(String((e as Error)?.message ?? e));
    } finally {
      setBusyId(null);
    }
  };

  const copyPath = async (path: string) => {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* 剪贴板不可用时不说什么：路径就在屏幕上，用户能自己选中 */
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* PageHeader 是纵向流的第一个孩子，必须 `shrink-0`：它一被压缩，
          下面的 grid 就挤上来，「左栏标题」会看起来长在右栏顶上（第一版就是这样）。 */}
      <div className="shrink-0">
        <PageHeader title={t("tools.hookTab")} description={t("tools.listHint")} />
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-[minmax(220px,280px)_minmax(0,1fr)] gap-3">
        {/* 左：气泡卡片列表 */}
        <aside className="flex min-h-0 min-w-0 flex-col gap-2" data-testid="market-list">
          <div className="flex shrink-0 items-center gap-1.5 px-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70">
            <Wrench className="h-3 w-3" />
            {t("tools.listTitle")}
            {tools.length > 0 && <Pill tone="muted">{tools.length}</Pill>}
          </div>
          <div className="pane-scroll min-h-0 flex-1 space-y-1.5 overflow-auto pr-0.5">
            {isLoading && (
              <p className="px-1 py-3 text-[11px] text-muted-foreground">{t("tools.loading")}</p>
            )}
            {!isLoading && tools.length === 0 && (
              <p className="px-1 py-3 text-[11px] text-muted-foreground">{t("tools.listEmpty")}</p>
            )}
            {tools.map((tool) => (
              <ToolBubble
                key={tool.id}
                tool={tool}
                selected={tool.id === selectedId}
                onSelect={() => {
                  setSelectedId(tool.id);
                  // 换工具就把上一个的安装回执收掉：那是另一个文件的结果
                  setReport(null);
                  setError(null);
                }}
              />
            ))}
          </div>
        </aside>

        {/* 右：操作区 */}
        <section
          className="pane-scroll min-h-0 min-w-0 overflow-auto rounded-2xl border border-border/60 bg-card/60 p-3 shadow-card"
          data-testid="market-detail"
        >
          {selected ? (
            <div className="flex flex-col gap-3">
              <header className="flex min-w-0 items-start gap-2.5">
                <IconTile icon={Wrench} />
                <div className="min-w-0 flex-1">
                  <h2 className="truncate text-sm font-semibold leading-tight">{selected.name}</h2>
                  <p className="mt-0.5 text-[11px] leading-tight text-muted-foreground">
                    {selected.tagline}
                  </p>
                </div>
              </header>

              {/* 事实行：版本 / 体积 / 许可，全是目录里登记的值，不是猜的 */}
              <div className="flex flex-wrap items-center gap-1.5">
                <Pill tone="mono" title={t("tools.version")}>
                  {selected.version}
                </Pill>
                <Pill tone="mono" title={t("tools.size")}>
                  {formatBytes(selected.sizeBytes)}
                </Pill>
                <Pill tone="muted" title={t("tools.license")}>
                  {selected.license}
                </Pill>
                {selected.deviceAbi.map((a) => (
                  <Pill key={a} tone="neutral">
                    {a}
                  </Pill>
                ))}
              </div>

              <section className="space-y-1">
                <h3 className="text-[11px] font-semibold">{t("tools.description")}</h3>
                <p className="text-[11px] leading-relaxed text-muted-foreground">
                  {selected.description}
                </p>
              </section>

              <section className="space-y-1">
                <h3 className="text-[11px] font-semibold">{t("tools.homepage")}</h3>
                <div className="flex items-center gap-1.5">
                  <a
                    href={selected.homepage}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="min-w-0 truncate font-mono text-[11px] text-primary hover:underline"
                    data-testid="market-homepage"
                  >
                    {selected.homepage}
                  </a>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    title={t("tools.openHomepage")}
                    onClick={() => window.open(selected.homepage, "_blank", "noopener")}
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                  </Button>
                </div>
              </section>

              <section className="space-y-1">
                <h3 className="text-[11px] font-semibold">{t("tools.usage")}</h3>
                <div className="space-y-1 rounded-xl border border-border/60 bg-muted/40 p-2">
                  {selected.usage.map((line) => (
                    <p key={line} className="path-selectable font-mono text-[11px] text-foreground/90">
                      {line}
                    </p>
                  ))}
                </div>
                <p className="text-[10px] leading-relaxed text-muted-foreground">
                  {t("tools.usageHint")}
                </p>
              </section>

              {/* 安装区 */}
              <section className="space-y-2 rounded-xl border border-border/60 bg-card p-3 shadow-card">
                <div className="flex flex-wrap items-center gap-2">
                  <select
                    className="h-8 min-w-0 flex-1 rounded-lg border border-input bg-background/60 px-2 text-[11px]"
                    value={deviceSerial ?? ""}
                    onChange={(event) => setDeviceSerial(event.target.value || null)}
                    aria-label={t("tools.pickDevice")}
                    data-testid="market-device-select"
                  >
                    {devices.length === 0 && <option value="">{t("tools.noDevice")}</option>}
                    {devices.map((d: DeviceEntry) => (
                      <option key={d.serial} value={d.serial}>
                        {d.model || d.serial} · {d.serial}
                      </option>
                    ))}
                  </select>
                  <Button
                    size="md"
                    disabled={!deviceSerial || busyId !== null || abiMismatch}
                    onClick={() => void install(selected)}
                    data-testid="market-install"
                    title={
                      abiMismatch
                        ? t("tools.abiMismatch", {
                            abi: abi ?? "",
                            toolAbi: selected.deviceAbi.join("/"),
                          })
                        : !deviceSerial
                          ? t("tools.needDeviceHint")
                          : undefined
                    }
                  >
                    {busyId === selected.id ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <PackageCheck className="h-3.5 w-3.5" />
                    )}
                    {busyId === selected.id
                      ? t("tools.installing")
                      : report
                        ? t("tools.reinstall")
                        : t("tools.install")}
                  </Button>
                </div>
                {!deviceSerial && (
                  <p className="text-[10px] text-muted-foreground">{t("tools.needDevice")}</p>
                )}
                {abiMismatch && (
                  <p className="text-[10px] text-destructive">
                    {t("tools.abiMismatch", {
                      abi: abi ?? "",
                      toolAbi: selected.deviceAbi.join("/"),
                    })}
                  </p>
                )}
                <p className="text-[10px] leading-relaxed text-muted-foreground">
                  {t("tools.downloadNote")}
                </p>

                {error && (
                  <p className="rounded-lg bg-destructive/10 px-2 py-1.5 text-[11px] text-destructive" data-testid="market-error">
                    {t("tools.installFailed", { error })}
                  </p>
                )}

                {report && (
                  <div className="space-y-1.5 rounded-lg bg-muted/50 p-2" data-testid="market-report">
                    <div className="flex items-center gap-1.5">
                      <PackageCheck className="h-3.5 w-3.5 text-emerald-500" />
                      <span className="text-[11px] font-medium">{t("tools.installed")}</span>
                      <Pill tone="mono">{report.version}</Pill>
                    </div>
                    <div className="flex items-center gap-1.5">
                      <span className="shrink-0 text-[10px] text-muted-foreground">
                        {t("tools.devicePath")}
                      </span>
                      <code className="min-w-0 flex-1 truncate rounded bg-background/70 px-1.5 py-0.5 font-mono text-[11px]">
                        {report.devicePath}
                      </code>
                      <Button
                        size="icon-sm"
                        variant="ghost"
                        title={copied ? t("tools.copied") : t("tools.copyPath")}
                        onClick={() => void copyPath(report.devicePath)}
                      >
                        <Copy className={cn("h-3.5 w-3.5", copied && "text-emerald-500")} />
                      </Button>
                    </div>
                    <div className="flex flex-wrap items-center gap-1.5">
                      {/* 复核结果分三档，不能合并：验过 / 没验过 / 验了但不一致 */}
                      {report.verifiedBytes === null ? (
                        <Pill tone="muted">{t("tools.notVerified")}</Pill>
                      ) : (
                        <Pill
                          tone={report.verifiedBytes === report.bytes ? "accent" : "muted"}
                          title={`${report.verifiedBytes} / ${report.bytes}`}
                        >
                          {t("tools.verified", { size: formatBytes(report.verifiedBytes) })}
                        </Pill>
                      )}
                      <Pill tone={report.executable ? "accent" : "muted"}>
                        {report.executable ? t("tools.executable") : t("tools.notExecutable")}
                      </Pill>
                    </div>
                  </div>
                )}
              </section>
            </div>
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-1 text-center">
              <IconTile icon={Wrench} />
              <p className="text-xs font-medium">{t("tools.selectHint")}</p>
              <p className="max-w-xs text-[11px] text-muted-foreground">
                {t("tools.selectHintDesc")}
              </p>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

/**
 * 一张气泡卡片。
 *
 * 选中态用「抬起 + 主色描边」而不是换底色：底色一变，卡片之间的相对关系就没了
 * （看起来像"这一张属于另一类"），而抬起只是"它现在被拿在手上"。
 * 圆角与内边距比表格行大一档，是有意的——这是气泡，不是行。
 */
function ToolBubble({
  tool,
  selected,
  onSelect,
}: {
  tool: MarketTool;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      data-testid={`market-tool-${tool.id}`}
      data-selected={selected ? "true" : undefined}
      className={cn(
        "group relative w-full rounded-2xl border px-3 py-2.5 text-left transition-colors",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        selected
          ? "border-primary/50 bg-primary/[0.08] shadow-card ring-1 ring-primary/20 dark:bg-primary/[0.12]"
          : "border-border/60 bg-card/80 shadow-row hover:border-border hover:bg-card",
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className={cn("truncate text-xs font-semibold", selected && "text-primary")}>
          {tool.name}
        </span>
        <Pill tone={selected ? "accent" : "muted"}>{tool.version}</Pill>
      </div>
      <p className="mt-0.5 line-clamp-2 text-[10px] leading-snug text-muted-foreground">
        {tool.tagline}
      </p>
      <div className="mt-1.5 flex items-center gap-1 text-[10px] text-muted-foreground/80">
        <span className="tabular-nums">{formatBytes(tool.sizeBytes)}</span>
        <span className="opacity-40">·</span>
        <span className="truncate">{tool.license}</span>
      </div>
    </button>
  );
}
