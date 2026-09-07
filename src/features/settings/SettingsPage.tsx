import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
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
import { deviceApi, type AdbEnvironment } from "@/api/device";
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

/** 设置页：主题、背景透明度、日志级别（P1 起全部持久化到 SQLite） */
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
        <Label>ADB 路径（P3）</Label>
        <AdbPathSection onProbed={() => {
          // adb 环境变了：仪表盘/设备页的查询立即失效重取
          void queryClient.invalidateQueries({ queryKey: ["adb"] });
          void queryClient.invalidateQueries({ queryKey: ["devices"] });
        }} />
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
