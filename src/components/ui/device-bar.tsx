/** 设备选择条 + Root 开关：托管页与进程端口页共用（原来寄存在 BinaryHosting 里，那一页搬到「二进制」主 tab 后两个使用者都不该去 import 它） */
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { type DeviceEntry } from "@/api/device";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export function DeviceBar({
  online,
  selected,
  onSelect,
  onRefresh,
  refreshing,
  root,
  probing,
  onRootChange,
}: {
  online: DeviceEntry[];
  selected: string | null;
  onSelect: (s: string) => void;
  onRefresh: () => void;
  refreshing: boolean;
  root: boolean;
  probing: boolean;
  onRootChange: (checked: boolean) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex shrink-0 items-center gap-2 text-xs">
      {online.length === 0 && <span className="text-muted-foreground">{t("adb.forward.rowInactive")}</span>}
      {online.length === 1 && (
        <span className="font-mono text-muted-foreground">-s {online[0].serial}</span>
      )}
      {online.length > 1 && (
        <label className="flex items-center gap-2">
          <span className="font-mono text-muted-foreground">-s</span>
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
      )}
      <label
        className={cn(
          "ml-auto flex shrink-0 items-center gap-1.5 text-muted-foreground",
          probing && "opacity-50",
        )}
        title={t("adb.binary.rootHint")}
      >
        <input
          type="checkbox"
          aria-label={t("adb.binary.rootLabel")}
          disabled={probing || !selected}
          checked={root}
          onChange={(e) => onRootChange(e.target.checked)}
        />
        Root (su)
      </label>
      <Button
        size="sm"
        variant="outline"
        className="h-7 gap-1 px-2"
        disabled={refreshing || !selected}
        onClick={onRefresh}
      >
        <RefreshCw className={cn("h-3 w-3", refreshing && "animate-spin")} />
        {t("common.refresh")}
      </Button>
    </div>
  );
}
