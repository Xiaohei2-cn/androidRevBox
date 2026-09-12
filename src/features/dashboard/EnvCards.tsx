import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, CircleSlash, RefreshCw, Settings2, XCircle } from "lucide-react";
import { envApi, type McpEnv } from "@/api/env";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { BrandIcon } from "@/components/ui/BrandIcon";
import { PathText } from "@/components/ui/PathText";
import { cn } from "@/lib/utils";

/**
 * 仪表盘环境/工具卡片（P7）：Python / Node / Frida / IDA MCP / jadx MCP。
 * 布局契约：左右两列网格，任何卡不得独占一行；状态三色——
 * 绿=就绪、琥珀=未配置/未检测到（常态，不算错误）、红=探测异常。
 * Python/Node 未配置时提供「去配置」按钮，跳转设置页并蓝框闪烁定位。
 */

export function PythonCard() {
  const { t } = useI18n();
  const { data, isFetching, refetch } = useEnvQuery(["env", "python"], envApi.python);
  return (
    <EnvCard
      testid="env-python"
      icon={<BrandIcon name="python" />}
      title={t("dashboard.python.title")}
      refresh={() => void refetch()}
      refreshing={isFetching}
      configKey="app.python.path"
      status={
        data === undefined
          ? { tone: "muted", label: t("common.loading") }
          : data.ready
            ? { tone: "ok", label: t("dashboard.python.version", { version: data.version ?? "" }) }
            : data.configured
              ? { tone: "warn", label: t("common.unavailable") }
              : { tone: "warn", label: t("common.notConfigured") }
      }
    >
      {data === undefined ? null : data.ready ? (
        <PathLine>
          <PathText value={data.path} testid="env-python-path" className="font-mono" />
        </PathLine>
      ) : (
        <PathLine>{data.hint}</PathLine>
      )}
    </EnvCard>
  );
}

export function NodeCard() {
  const { t } = useI18n();
  const { data, isFetching, refetch } = useEnvQuery(["env", "node"], envApi.node);
  return (
    <EnvCard
      testid="env-node"
      icon={<BrandIcon name="node" />}
      title={t("dashboard.node.title")}
      refresh={() => void refetch()}
      refreshing={isFetching}
      configKey="app.node.path"
      status={
        data === undefined
          ? { tone: "muted", label: t("common.loading") }
          : data.ready
            ? { tone: "ok", label: t("dashboard.node.version", { version: data.version ?? "" }) }
            : { tone: "warn", label: t("common.notDetected") }
      }
    >
      {data === undefined ? null : data.ready ? (
        <>
          <PathLine label={t("dashboard.node.nodeLabel")}>
            <PathText value={data.path} testid="env-node-path" className="font-mono" />
          </PathLine>
          <PathLine label={t("dashboard.node.globalRoot")}>
            <PathText
              value={data.npmGlobalRoot}
              testid="env-node-npmroot"
              className="font-mono"
            />
          </PathLine>
        </>
      ) : (
        <PathLine>{data.hint}</PathLine>
      )}
    </EnvCard>
  );
}

/** Frida 卡依赖 Python 就绪（§10 剪枝：未就绪不发起 env_frida 查询） */
export function FridaCard({ pythonReady }: { pythonReady: boolean | undefined }) {
  const { t } = useI18n();
  const enabled = pythonReady === true;
  const { data, isFetching, refetch } = useEnvQuery(["env", "frida"], envApi.frida, enabled);
  return (
    <EnvCard
      testid="env-frida"
      icon={<BrandIcon name="frida" />}
      title={t("dashboard.frida.title")}
      refresh={() => void refetch()}
      refreshing={isFetching}
      disabled={!enabled}
      configKey={enabled ? undefined : "app.python.path"}
      status={
        !enabled
          ? { tone: "muted", label: t("dashboard.frida.waitPython") }
          : data === undefined
            ? { tone: "muted", label: t("common.loading") }
            : data.installed
              ? { tone: "ok", label: t("common.installed") }
              : { tone: "warn", label: t("common.notInstalled") }
      }
    >
      {!enabled ? (
        <PathLine>{t("dashboard.frida.hintConfig")}</PathLine>
      ) : data === undefined ? null : data.installed ? (
        <PathLine mono>
          frida <PathText value={data.fridaVersion} testid="env-frida-version" className="font-mono" />
          {data.fridaToolsVersion
            ? ` · frida-tools ${data.fridaToolsVersion}`
            : ""}
        </PathLine>
      ) : (
        <PathLine>{data.hint}</PathLine>
      )}
    </EnvCard>
  );
}

/** IDA / jadx MCP 状态卡（同形组件，端口可配置；连不上是常态不打红） */
export function McpCard({
  testid,
  title,
  queryFn,
  configKey,
  brand,
}: {
  testid: string;
  title: string;
  queryFn: () => Promise<McpEnv>;
  configKey: string;
  /** 用哪个官方图标标识该工具 */
  brand: "ida" | "jadx";
}) {
  const { t } = useI18n();
  const { data, isFetching, refetch } = useEnvQuery(["env", testid], queryFn);
  return (
    <EnvCard
      testid={testid}
      icon={<BrandIcon name={brand} />}
      title={title}
      refresh={() => void refetch()}
      refreshing={isFetching}
      configKey={configKey}
      status={
        data === undefined
          ? { tone: "muted", label: t("common.loading") }
          : data.reachable
            ? { tone: "ok", label: t("dashboard.mcp.online", { port: data.port }) }
            : { tone: "warn", label: t("common.notDetected") }
      }
    >
      {data === undefined ? null : (
        <>
          <PathLine>
            <PathText
              value={`127.0.0.1:${data.port}`}
              testid={`${testid}-addr`}
              className="font-mono"
            />
          </PathLine>
          {data.appInstalled !== null && data.appInstalled !== undefined && (
            <PathLine>
              {data.appInstalled ? (
                <span className="text-emerald-500">✓ {data.appPath ?? brand}</span>
              ) : (
                <span className="text-amber-500">
                  {t("dashboard.mcp.appMissing", { name: brand })}
                </span>
              )}
            </PathLine>
          )}
          {!data.reachable && <PathLine>{data.hint}</PathLine>}
        </>
      )}
    </EnvCard>
  );
}

// ===== 通用骨架 =====

export type StatusTone = "ok" | "warn" | "muted";

export function EnvCard({
  testid,
  icon,
  title,
  status,
  refresh,
  refreshing,
  disabled,
  configKey,
  children,
}: {
  testid: string;
  /** 卡片图标：品牌图片（BrandIcon）或 lucide 组件，统一渲染为 16×16 */
  icon: React.ReactNode;
  title: string;
  status: { tone: StatusTone; label: string };
  refresh: () => void;
  refreshing: boolean;
  disabled?: boolean;
  /** 提供后，标题行出现「去配置」按钮，点击跳转设置页并高亮该配置项 */
  configKey?: string;
  children?: React.ReactNode;
}) {
  const { gotoConfig } = useAppNav();
  const { t } = useI18n();
  return (
    <div data-testid={testid} className="rounded-xl border bg-card p-4" aria-disabled={disabled}>
      <div className="flex items-center gap-2">
        <span className="flex h-4 w-4 shrink-0 items-center justify-center text-muted-foreground">
          {icon}
        </span>
        <span className="truncate text-sm font-semibold">{title}</span>
        {configKey && (
          <button
            type="button"
            aria-label={t("common.configure", { name: title })}
            title={t("common.gotoSettings")}
            onClick={() => gotoConfig(configKey)}
            className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
            data-testid={`${testid}-goto-config`}
          >
            <Settings2 className="h-3.5 w-3.5" />
          </button>
        )}
        <button
          type="button"
          aria-label={t("common.refreshName", { name: title })}
          disabled={refreshing || disabled}
          onClick={refresh}
          className="ml-auto rounded p-1 text-muted-foreground hover:bg-accent disabled:opacity-30"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", refreshing && "animate-spin")} />
        </button>
      </div>
      <div className="mt-2 flex items-center gap-1.5 text-sm font-medium">
        <StatusMark tone={status.tone} />
        <span data-testid={`${testid}-status`}>{status.label}</span>
      </div>
      <div className="mt-1.5 space-y-1">{children}</div>
    </div>
  );
}

function StatusMark({ tone }: { tone: StatusTone }) {
  if (tone === "ok") return <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500" />;
  if (tone === "warn") return <XCircle className="h-3.5 w-3.5 text-amber-500" />;
  return <CircleSlash className="h-3.5 w-3.5 text-muted-foreground" />;
}

function PathLine({
  label,
  mono,
  children,
}: {
  label?: string;
  mono?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <p
      className={cn(
        "flex items-baseline gap-1.5 text-xs leading-relaxed text-muted-foreground",
        mono && "font-mono",
      )}
    >
      {label && <span className="shrink-0">{label}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
    </p>
  );
}

function useEnvQuery<T>(
  key: readonly unknown[],
  queryFn: () => Promise<T>,
  enabled = true,
) {
  return useQuery({
    queryKey: key,
    queryFn,
    enabled,
    staleTime: 30_000,
    retry: false,
  });
}

/** 供设置页改动后整体失效环境查询 */
export function useInvalidateEnvQueries() {
  const qc = useQueryClient();
  return () => void qc.invalidateQueries({ queryKey: ["env"] });
}
