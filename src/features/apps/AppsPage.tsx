import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type DeviceEntry } from "@/api/device";
import { AppsView } from "@/features/devices/DevicesPage";
import { useActiveTab } from "@/app/nav";
import { useI18n } from "@/i18n";

/** 主 tab 的应用清单入口：设备选择与 Zygisk 应用能力分离。 */
export function AppsPage() {
  const active = useActiveTab("apps");
  const { t } = useI18n();
  const [selected, setSelected] = useState<string | null>(null);
  const { data: devices = [], isLoading, refetch } = useQuery<DeviceEntry[]>({
    queryKey: ["apps", "devices"],
    queryFn: () => deviceApi.list(true),
    enabled: active,
    refetchInterval: active ? 10_000 : false,
  });
  const serial = useMemo(() => {
    if (devices.some((device) => device.serial === selected)) return selected;
    return devices[0]?.serial ?? null;
  }, [devices, selected]);

  if (isLoading) return <div className="flex h-full items-center justify-center text-xs text-muted-foreground">{t("apps.devicesLoading")}</div>;
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 items-center gap-2">
        <span className="text-sm font-medium">{t("apps.title")}</span>
        <select
          value={serial ?? ""}
          onChange={(event) => setSelected(event.target.value || null)}
          className="h-8 min-w-56 rounded-md border border-input bg-transparent px-2 text-xs"
          aria-label={t("apps.selectDevice")}
        >
          <option value="">{t("apps.noDevice")}</option>
          {devices.map((device) => (
            <option key={device.serial} value={device.serial}>
              {device.model || device.serial} · {device.serial}
            </option>
          ))}
        </select>
        <Button size="sm" variant="outline" onClick={() => void refetch()}>
          <RefreshCw className="h-3.5 w-3.5" />
          {t("apps.refreshDevices")}
        </Button>
      </div>
      <div className="min-h-0 flex-1">
        <AppsView serial={serial} />
      </div>
    </div>
  );
}
