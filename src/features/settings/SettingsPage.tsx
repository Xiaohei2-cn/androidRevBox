import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen } from "lucide-react";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SubTabs } from "@/components/nav/SubTabs";
import {
  MIN_OPACITY,
  useSettings,
  type LogLevel,
  type ThemePref,
} from "@/app/providers";
import { useAppNav } from "@/app/nav";
import { deviceApi, type AdbEnvironment } from "@/api/device";
import { systemApi } from "@/api/system";
import { configApi } from "@/api/config";
import { pickFile } from "@/api/dialog";
import { LOCALES, LOCALE_LABELS, useI18n } from "@/i18n";
import type { Locale } from "@/i18n/dictionaries";
import { cn } from "@/lib/utils";

const THEME_OPTIONS: { value: ThemePref }[] = [
  { value: "light" },
  { value: "dark" },
  { value: "system" },
];

const LOG_LEVEL_OPTIONS: { value: LogLevel; label: string }[] = [
  { value: "trace", label: "trace" },
  { value: "debug", label: "debug" },
  { value: "info", label: "info" },
  { value: "warn", label: "warn" },
  { value: "error", label: "error" },
];

/**
 * 设置页（P8 改版）：按职能分 tab——显示 / 语言 / 环境 / 系统。
 * 分 tab 复用通用 SubTabs 机制；「去配置」跳转的目标键所在 tab 由 ConfigInputRow
 * 自身渲染（各 tab 都挂载，Radix Tabs 未激活内容仍在 DOM，聚焦定位天然可用）。
 */
export function SettingsPage() {
  const { t } = useI18n();
  const { pendingConfigKey } = useAppNav();

  // 「去配置」跨 tab 定位：目标键落在环境 tab（未来新键按前缀归 tab）
  const targetTab = pendingConfigKey?.startsWith("app.tools.") ||
      pendingConfigKey === "app.python.path" ||
      pendingConfigKey === "app.node.path" ||
      pendingConfigKey === "app.adb.path"
    ? "environment"
    : null;

  return (
    <div className="h-full pt-1">
      <SubTabs
        activateSignal={targetTab}
        tabs={[
          {
            id: "display",
            label: t("settings.tab.display"),
            content: <DisplayTab />,
          },
          {
            id: "language",
            label: t("settings.tab.language"),
            content: <LanguageTab />,
          },
          {
            id: "environment",
            label: t("settings.tab.environment"),
            content: <EnvironmentTab />,
          },
          {
            id: "system",
            label: t("settings.tab.system"),
            content: <SystemTab />,
          },
        ]}
      />
    </div>
  );
}

/** 显示：主题 + 背景不透明度 */
function DisplayTab() {
  const { theme, setTheme, opacity, setOpacity } = useSettings();
  const { t } = useI18n();

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-8 pt-2">
      <section className="flex flex-col gap-3">
        <Label>{t("settings.theme.label")}</Label>
        <div
          className="inline-flex w-fit rounded-lg bg-muted p-1"
          role="radiogroup"
          aria-label={t("settings.theme.aria")}
        >
          {THEME_OPTIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={theme === option.value}
              onClick={() => setTheme(option.value)}
              className={cn(
                "rounded-md px-4 py-1.5 text-sm transition-colors",
                theme === option.value
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {t(`settings.theme.${option.value}`)}
            </button>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">{t("settings.theme.hint")}</p>
      </section>

      <section className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <Label htmlFor="opacity-slider">{t("settings.opacity.label")}</Label>
          <span className="text-sm tabular-nums text-muted-foreground">
            {opacity}%
          </span>
        </div>
        <Slider
          id="opacity-slider"
          min={MIN_OPACITY}
          max={100}
          step={5}
          value={[opacity]}
          onValueChange={(values) => setOpacity(values[0])}
        />
        <p className="text-xs text-muted-foreground">{t("settings.opacity.hint")}</p>
      </section>
    </div>
  );
}

/** 语言：界面语种切换 */
function LanguageTab() {
  const { t, locale, setLocale } = useI18n();

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-3 pt-2">
      <Label>{t("settings.language.label")}</Label>
      <div
        className="inline-flex w-fit flex-wrap gap-1"
        role="radiogroup"
        aria-label={t("settings.language.label")}
        data-testid="locale-radiogroup"
      >
        {LOCALES.map((l) => (
          <button
            key={l}
            type="button"
            role="radio"
            aria-checked={locale === l}
            data-testid={`locale-${l}`}
            onClick={() => setLocale(l as Locale)}
            className={cn(
              "rounded-md border px-3 py-1.5 text-xs transition-colors",
              locale === l
                ? "border-primary bg-primary text-primary-foreground"
                : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
          >
            {LOCALE_LABELS[l]}
          </button>
        ))}
      </div>
      <p className="text-xs text-muted-foreground">{t("settings.language.hint")}</p>
    </div>
  );
}

/** 环境：ADB 路径 + 工具链（Python/Node/MCP 端口） */
function EnvironmentTab() {
  const { t } = useI18n();
  const queryClient = useQueryClient();

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-8 pt-2">
      <section className="flex flex-col gap-3">
        <Label>{t("settings.adb.label")}</Label>
        <AdbPathSection onProbed={() => {
          // adb 环境变了：仪表盘/设备页的查询立即失效重取
          void queryClient.invalidateQueries({ queryKey: ["adb"] });
          void queryClient.invalidateQueries({ queryKey: ["devices"] });
        }} />
      </section>

      <section className="flex flex-col gap-3">
        <Label>{t("settings.tools.label")}</Label>
        <p className="text-xs text-muted-foreground">{t("settings.tools.hint")}</p>
        <ConfigInputRow
          label={t("settings.tools.pythonPath")}
          configKey="app.python.path"
          placeholder={t("settings.tools.pythonPath.placeholder")}
          mono
          kind="file"
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label={t("settings.tools.nodePath")}
          configKey="app.node.path"
          placeholder={t("settings.tools.nodePath.placeholder")}
          mono
          kind="file"
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label={t("settings.tools.idaPort")}
          configKey="app.tools.ida_mcp_port"
          placeholder={t("settings.tools.idaPort.placeholder")}
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label={t("settings.tools.jadxPort")}
          configKey="app.tools.jadx_mcp_port"
          placeholder={t("settings.tools.jadxPort.placeholder")}
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
      </section>
    </div>
  );
}

/** 系统：日志级别 + 关于 */
function SystemTab() {
  const { logLevel, setLogLevel, hydrated } = useSettings();
  const { t } = useI18n();

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-8 pt-2">
      <section className="flex flex-col gap-3">
        <Label>{t("settings.log.label")}</Label>
        <div className="inline-flex w-fit gap-1">
          {LOG_LEVEL_OPTIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={logLevel === option.value}
              onClick={() => setLogLevel(option.value)}
              className={cn(
                "rounded-md border px-3 py-1.5 font-mono text-xs transition-colors",
                logLevel === option.value
                  ? "border-primary bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.log.hint")}
          {!hydrated && t("settings.log.syncing")}
        </p>
      </section>

      <AboutSection />
    </div>
  );
}

/** 通用配置行：从后端 snapshot 回显初值，点「应用」落库（后端做键白名单 + 值校验）。
 *  从卡片「去配置」跳转进来时：自动滚动到位、聚焦输入框、蓝框闪烁两下。 */
function ConfigInputRow({
  label,
  configKey,
  placeholder,
  mono,
  kind,
  onSaved,
}: {
  label: string;
  configKey: string;
  placeholder?: string;
  mono?: boolean;
  /** 路径类输入显示「打开文件夹选择器」按钮：directory=选目录，file=选文件 */
  kind?: "directory" | "file";
  onSaved: () => void;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [flash, setFlash] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const { pendingConfigKey, clearPendingConfig } = useAppNav();
  const { t } = useI18n();
  const [picking, setPicking] = useState(false);

  // 原生选择器（浏览器 dev 降级为 no-op 返回 null，保持手填可用）
  const browse = async () => {
    setPicking(true);
    try {
      const isPython = configKey === "app.python.path";
      const defaultPath = isPython ? await guessPythonStartDir() : undefined;
      const picked = await pickFile(
        kind === "file"
          ? { title: label, defaultPath }
          : { title: label, defaultPath },
      );
      if (picked && isPython) {
        // pyenv shim / .app 包入口 → 后端解析成真解释器路径并回填输入框
        try {
          const { envApi } = await import("@/api/env");
          const resolved = await envApi.resolveInterpreter(picked);
          if (resolved?.resolvedPath) setValue(resolved.resolvedPath);
          else setValue(picked);
        } catch {
          setValue(picked);
        }
      } else if (picked) {
        setValue(picked);
      }
    } catch {
      // 选择器异常不阻塞手填
    } finally {
      setPicking(false);
    }
  };

  // 初值回显：从 snapshot 里找当前键（缺失 = 用默认）
  const { data: snapshot } = useQuery({
    queryKey: ["config", "snapshot"],
    queryFn: configApi.snapshot,
    staleTime: 60_000,
  });
  useEffect(() => {
    const row = snapshot?.find((s) => s.key === configKey);
    if (row !== undefined) setValue(row.value);
  }, [snapshot, configKey]);

  useEffect(() => {
    if (pendingConfigKey !== configKey) return;
    const el = inputRef.current;
    if (!el) return;
    el.closest("div")?.scrollIntoView({ behavior: "smooth", block: "center" });
    el.focus();
    setFlash(true);
    const timer = window.setTimeout(() => {
      setFlash(false);
      clearPendingConfig();
    }, 1300); // 动画 0.6s × 2 遍
    return () => window.clearTimeout(timer);
  }, [pendingConfigKey, configKey, clearPendingConfig]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await configApi.set(configKey, value.trim());
      setSaved(true);
      onSaved();
      window.setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError(String((e as { message?: string }).message ?? e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <Label className="w-36 shrink-0 text-xs font-normal text-muted-foreground">
          {label}
        </Label>
        <Input
          ref={inputRef}
          value={value}
          placeholder={placeholder}
          onChange={(e) => setValue(e.target.value)}
          className={cn(
            "h-8 flex-1 text-xs",
            mono && "font-mono",
            flash && "config-flash",
          )}
          data-testid={`config-${configKey}`}
        />
        {kind && (
          <Button
            size="sm"
            variant="outline"
            className="h-8 w-8 p-0"
            disabled={picking}
            aria-label={t("settings.tools.browse")}
            title={t("settings.tools.browse")}
            data-testid={`browse-${configKey}`}
            onClick={() => void browse()}
          >
            <FolderOpen className="h-3.5 w-3.5" />
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void save()}>
          {t("common.apply")}
        </Button>
      </div>
      {error && <p className="pl-36 text-xs text-destructive">{error}</p>}
      {saved && !error && (
        <p className="pl-36 text-xs text-emerald-500">{t("settings.tools.savedReload")}</p>
      )}
    </div>
  );
}

/** 关于（P7）：应用版本 / Tauri 版本 / 运行平台，自仪表盘系统四卡迁入 */
function AboutSection() {
  const { t } = useI18n();
  const { data } = useQuery({
    queryKey: ["system", "ping"],
    queryFn: systemApi.ping,
    staleTime: Infinity,
  });

  return (
    <section className="flex flex-col gap-3">
      <Label>{t("settings.about.label")}</Label>
      <dl data-testid="about-section" className="space-y-1.5 text-xs">
        <AboutRow label={t("settings.about.appVersion")} value={data?.appVersion} testid="about-app-version" />
        <AboutRow label={t("settings.about.tauriVersion")} value={data?.tauriVersion} testid="about-tauri-version" />
        <AboutRow
          label={t("settings.about.platform")}
          value={data ? `${data.os} · ${data.arch}` : undefined}
          testid="about-platform"
        />
      </dl>
    </section>
  );
}

function AboutRow({
  label,
  value,
  testid,
}: {
  label: string;
  value?: string;
  testid: string;
}) {
  return (
    <div className="flex items-center gap-3">
      <dt className="w-36 shrink-0 text-muted-foreground">{label}</dt>
      <dd data-testid={testid} className="font-mono">
        {value ?? "—"}
      </dd>
    </div>
  );
}

/** ADB 路径设置：空=自动探测（ANDROID_HOME/SDK_ROOT/PATH），手动配置优先 */
function AdbPathSection({ onProbed }: { onProbed: () => void }) {
  const { t } = useI18n();
  const [env, setEnv] = useState<AdbEnvironment | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);

  const browse = async () => {
    setPicking(true);
    try {
      const picked = await pickFile({
        title: t("settings.adb.label"),
        filters: [{ name: "adb", extensions: process.platform === "win32" ? ["exe"] : ["*"] }],
      });
      if (picked) setPath(picked);
    } catch {
      // 选择器异常不阻塞手填
    } finally {
      setPicking(false);
    }
  };

  const apply = async (value: string) => {
    setBusy(true);
    setError(null);
    try {
      const next = await deviceApi.setPath(value.trim());
      setEnv(next);
      onProbed();
    } catch (e) {
      setError(String((e as { message?: string }).message ?? e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <Input
          value={path}
          placeholder={t("settings.adb.placeholder")}
          onChange={(e) => setPath(e.target.value)}
          className="font-mono"
        />
        <Button
          size="sm"
          variant="outline"
          className="h-8 w-8 shrink-0 p-0"
          disabled={picking}
          aria-label={t("settings.tools.browse")}
          title={t("settings.tools.browse")}
          data-testid="browse-app.adb.path"
          onClick={() => void browse()}
        >
          <FolderOpen className="h-3.5 w-3.5" />
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void apply(path)}>
          {t("common.apply")}
        </Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void apply("")}>
          {t("common.auto")}
        </Button>
      </div>
      {error && <p className="text-xs text-destructive">{error}</p>}
      {env && (
        <p className={cn("text-xs", env.installed ? "text-emerald-500" : "text-amber-500")}>
          {env.installed
            ? `✓ adb ${env.version} · ${env.path}`
            : env.hint ?? "未检测到 adb"}
        </p>
      )}
    </div>
  );
}


/** python 选择器起始目录：优先探测 pyenv/conda/framework 的 bin，依次回退到 /usr/bin */
async function guessPythonStartDir(): Promise<string | undefined> {
  if (!("__TAURI_INTERNALS__" in window)) return undefined;
  try {
    const { invokeCommand } = await import("@/api/client");
    return await invokeCommand<string>("env_python_start_dir");
  } catch {
    return undefined;
  }
}

