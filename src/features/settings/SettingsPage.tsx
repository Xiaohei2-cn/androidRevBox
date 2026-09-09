import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
import { cn } from "@/lib/utils";

const THEME_OPTIONS: { value: ThemePref; label: string }[] = [
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
  { value: "system", label: "跟随系统" },
];

const LOG_LEVEL_OPTIONS: { value: LogLevel; label: string }[] = [
  { value: "trace", label: "trace" },
  { value: "debug", label: "debug" },
  { value: "info", label: "info" },
  { value: "warn", label: "warn" },
  { value: "error", label: "error" },
];

/** 设置页：外观、ADB、工具环境（P7）、日志级别、关于（P7 自仪表盘迁入） */
export function SettingsPage() {
  const { theme, setTheme, opacity, setOpacity, logLevel, setLogLevel, hydrated } =
    useSettings();
  const queryClient = useQueryClient();

  return (
    <div className="mx-auto flex h-full max-w-xl flex-col gap-8 overflow-auto pt-4">
      <section className="flex flex-col gap-3">
        <Label>外观主题</Label>
        <div
          className="inline-flex w-fit rounded-lg bg-muted p-1"
          role="radiogroup"
          aria-label="外观主题"
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
              {option.label}
            </button>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">
          「跟随系统」会实时响应系统深浅色切换
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <Label htmlFor="opacity-slider">背景不透明度</Label>
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
        <p className="text-xs text-muted-foreground">
          调节窗口背景透明度，并叠加毛玻璃效果（macOS Vibrancy / Windows
          Acrylic；Linux 无合成器时自动降级为纯透明度）。最低 20%，保证内容可读。
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <Label>ADB 路径</Label>
        <AdbPathSection onProbed={() => {
          // adb 环境变了：仪表盘/设备页的查询立即失效重取
          void queryClient.invalidateQueries({ queryKey: ["adb"] });
          void queryClient.invalidateQueries({ queryKey: ["devices"] });
        }} />
      </section>

      <section className="flex flex-col gap-3">
        <Label>工具环境</Label>
        <p className="text-xs text-muted-foreground">
          仪表盘环境卡片使用；改动保存后自动重新探测。
        </p>
        <ConfigInputRow
          label="Python 解释器路径"
          configKey="app.python.path"
          placeholder="留空 = 未配置（Frida 检测将暂停）；如 /usr/bin/python3"
          mono
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label="Node 路径"
          configKey="app.node.path"
          placeholder="留空 = 自动探测系统 PATH 上的 node"
          mono
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label="IDA MCP 端口"
          configKey="app.tools.ida_mcp_port"
          placeholder="默认 13337"
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
        <ConfigInputRow
          label="jadx-gui MCP 端口"
          configKey="app.tools.jadx_mcp_port"
          placeholder="默认 8650"
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["env"] })}
        />
      </section>

      <section className="flex flex-col gap-3">
        <Label>日志级别</Label>
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
          运行时生效并持久化（SQLite app_settings），重启自动恢复。
          {!hydrated && " 配置同步中…"}
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
  onSaved,
}: {
  label: string;
  configKey: string;
  placeholder?: string;
  mono?: boolean;
  onSaved: () => void;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [flash, setFlash] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const { pendingConfigKey, clearPendingConfig } = useAppNav();

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
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void save()}>
          应用
        </Button>
      </div>
      {error && <p className="pl-36 text-xs text-destructive">{error}</p>}
      {saved && !error && (
        <p className="pl-36 text-xs text-emerald-500">已保存，环境卡重新检测中</p>
      )}
    </div>
  );
}

/** 关于（P7）：应用版本 / Tauri 版本 / 运行平台，自仪表盘系统四卡迁入 */
function AboutSection() {
  const { data } = useQuery({
    queryKey: ["system", "ping"],
    queryFn: systemApi.ping,
    staleTime: Infinity,
  });

  return (
    <section className="flex flex-col gap-3">
      <Label>关于</Label>
      <dl data-testid="about-section" className="space-y-1.5 text-xs">
        <AboutRow label="应用版本" value={data?.appVersion} testid="about-app-version" />
        <AboutRow label="Tauri 版本" value={data?.tauriVersion} testid="about-tauri-version" />
        <AboutRow
          label="运行平台"
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
  const [env, setEnv] = useState<AdbEnvironment | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
          placeholder="留空自动探测；或填 adb 可执行文件完整路径"
          onChange={(e) => setPath(e.target.value)}
          className="font-mono"
        />
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void apply(path)}>
          应用
        </Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void apply("")}>
          自动
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
