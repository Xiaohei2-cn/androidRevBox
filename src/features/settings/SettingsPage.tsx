import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { MIN_OPACITY, useSettings, type ThemePref } from "@/app/providers";
import { cn } from "@/lib/utils";

const THEME_OPTIONS: { value: ThemePref; label: string }[] = [
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
  { value: "system", label: "跟随系统" },
];

/** 设置页：P0 唯一真正可用的页面（主题 + 背景透明度） */
export function SettingsPage() {
  const { theme, setTheme, opacity, setOpacity } = useSettings();

  return (
    <div className="mx-auto flex h-full max-w-xl flex-col gap-8 overflow-auto pt-4">
      <section className="flex flex-col gap-3">
        <Label>外观主题</Label>
        <div className="inline-flex w-fit rounded-lg bg-muted p-1" role="radiogroup" aria-label="外观主题">
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
          调节窗口背景透明度，并叠加系统毛玻璃效果（macOS Vibrancy / Windows
          Acrylic；Linux 无合成器时自动降级为纯透明度）。最低 20%，保证内容可读。
        </p>
      </section>
    </div>
  );
}
