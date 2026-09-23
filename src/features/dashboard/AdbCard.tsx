import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, ChevronRight, XCircle, Usb } from "lucide-react";
import {
  deviceApi,
  type AdbEnvironment,
  type DeviceEntry,
} from "@/api/device";
import { useActiveTab, useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { BrandIcon } from "@/components/ui/BrandIcon";
import { PathText } from "@/components/ui/PathText";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { StatusBadge } from "@/components/ui/status-badge";

/**
 * 仪表盘 adb 卡片：环境指示（含未配置提示）、版本查看、设备连接轮询。
 * 后端 watch 线程经 device://changed 事件推热插拔；本卡片同时保留 10s 兜底轮询。
 * keep-mounted 后仅在仪表盘激活时轮询（§10 减少后台开销）。
 */
export function AdbCard() {
  const [events, setEvents] = useState(0);
  const active = useActiveTab("dashboard");
  const { t } = useI18n();
  const { gotoDeviceList } = useAppNav();

  const { data: env } = useQuery<AdbEnvironment>({
    queryKey: ["adb", "environment", events],
    queryFn: deviceApi.environment,
    staleTime: 5_000,
  });

  const { data: devices = [], isError: devicesError } = useQuery<DeviceEntry[]>({
    queryKey: ["adb", "devices", events],
    queryFn: () => deviceApi.list(),
    enabled: !!env?.installed && active,
    // 有事件时立即刷新，否则 10s 兜底轮询（仅激活时）
    refetchInterval: active ? 10_000 : false,
  });

  // 热插拔事件：bump queryKey 触发列表刷新（watch 线程在 Rust 侧，天然独立于渲染）
  useEffect(() => {
    let alive = true;
    const unsubs: Array<() => void> = [];
    deviceApi
      .onChanged(() => alive && setEvents((n) => n + 1))
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    return () => {
      alive = false;
      unsubs.forEach((un) => un());
    };
  }, []);

  const ready = devices.filter((d) => d.state === "device");
  const others = devices.filter((d) => d.state !== "device");

  return (
    <Card className="flex flex-col">
      <CardHeader>
        <BrandIcon name="android" />
        <CardTitle>{t("dashboard.adb.title")}</CardTitle>
        <span className="ml-auto" data-testid="adb-status">
          {env === undefined ? (
            <StatusBadge tone="muted">{t("common.loading")}</StatusBadge>
          ) : env.installed ? (
            <StatusBadge tone="ok">
              <CheckCircle2 className="mr-1 h-3 w-3" />
              {t("common.ready")}
            </StatusBadge>
          ) : (
            <StatusBadge tone="error">
              <XCircle className="mr-1 h-3 w-3" />
              {t("common.notDetected")}
            </StatusBadge>
          )}
        </span>
      </CardHeader>
      <CardContent className="mt-1 flex flex-1 flex-col">
        {env?.installed ? (
          <div className="space-y-1 text-xs text-muted-foreground">
            <p data-testid="adb-version" className="font-mono">
              adb {env.version}
              {env.build ? ` · ${env.build}` : ""}
            </p>
            <p className="flex items-center gap-1 font-mono text-xs">
              <PathText value={env.path} className="min-w-0 flex-1" />
              <span className="shrink-0 rounded bg-muted px-1.5 py-px text-xs">
                {SOURCE_LABEL[env.source ?? "unknown"] ?? env.source}
              </span>
            </p>
            {env.probeError && (
              <p className="text-amber-500">探测异常：{env.probeError}</p>
            )}
          </div>
        ) : (
          <p data-testid="adb-hint" className="text-xs text-amber-500">
            {env?.hint ?? "正在检测 adb 环境变量…"}
          </p>
        )}

        <div className="mt-3 border-t pt-3">
          <div className="flex items-center gap-1.5 text-xs">
            <Usb className="h-3.5 w-3.5 text-muted-foreground" />
            <span data-testid="adb-device-count" className="font-medium">
              {env?.installed
              ? t("dashboard.adb.connected", { count: ready.length })
              : t("dashboard.adb.paused")}
            </span>
            {others.length > 0 && (
              <span className="text-amber-500">
                {t("dashboard.adb.others", { count: others.length })}
              </span>
            )}
          </div>
          {devicesError && (
            <p className="mt-1 text-xs text-destructive">{t("dashboard.adb.listFailed")}</p>
          )}
          {ready.length > 0 && (
            <ul className="mt-2 space-y-1">
              {ready.slice(0, 4).map((d) => (
                <li key={d.serial} className="flex items-center gap-2 text-xs">
                  <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                  <span className="font-mono">{d.model || d.serial}</span>
                  <span className="text-muted-foreground">
                    <PathText value={d.serial} />
                  </span>
                  <StatusBadge tone="muted" className="px-1.5 py-0">
                    {d.transport}
                  </StatusBadge>
                  <button
                    type="button"
                    aria-label={t("devices.gotoList")}
                    title={t("devices.gotoList")}
                    data-testid={`goto-devices-${d.serial}`}
                    onClick={() => gotoDeviceList()}
                    className="ml-auto flex items-center rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                  >
                    <ChevronRight className="h-3.5 w-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

const SOURCE_LABEL: Record<string, string> = {
  config: "手动配置",
  android_home: "ANDROID_HOME",
  sdk_root: "ANDROID_SDK_ROOT",
  path_env: "PATH",
  mock: "mock",
};
