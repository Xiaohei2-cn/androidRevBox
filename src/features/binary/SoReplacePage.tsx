import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderInput, Play, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type DeviceEntry, type ReplaceNativeLibraryResult } from "@/api/device";
import { envApi } from "@/api/env";
import { pickFile } from "@/api/dialog";
import { useDragDropPath } from "@/hooks/useDragDropPath";
import { useActiveTab } from "@/app/nav";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * SO 替换（二进制主标签子页）：把修补后的 .so 写回应用安装目录，免重打包。
 * 流程（AR8.4 起由设备侧 Agent 执行）：① push 到唯一暂存目录；② Agent 推导目标路径、
 * 备份原件、同目录临时名落盘（权限/属主/SELinux 上下文跟随原件）→ rename → sha256 复核；
 * ③ 任一步失败自动回滚。页面展示**步骤链**而不是「一句话成功」，因为写安装目录这件事
 * 必须能回答「到底哪一步做了、原件还在不在」。执行前请先停止目标应用（运行中覆写会 text file busy）。
 */

type Abi = "arm64" | "arm";

export function SoReplacePage() {
  const { t } = useI18n();
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [localPath, setLocalPath] = useState("");
  const [pkg, setPkg] = useState("");
  const [abi, setAbi] = useState<Abi>("arm64");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ReplaceNativeLibraryResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const adbReady = !!env?.installed;

  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "soreplace"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d: DeviceEntry) => d.state === "device"), [devices]);
  useEffect(() => {
    if (!deviceSerial && online.length > 0) setDeviceSerial(online[0].serial);
  }, [deviceSerial, online]);

  const soName = localPath.split(/[\\/]/).pop() ?? "";
  const binaryActive = useActiveTab("binary");
  const onDropSo = useCallback((path: string) => {
    setLocalPath(path);
    setResult(null);
    setNotice(null);
  }, []);
  useDragDropPath({
    onPath: onDropSo,
    extensions: useMemo(() => ["so"], []),
    enabled: binaryActive,
  });
  const canPreview = !!deviceSerial && /^[A-Za-z0-9._-]+$/.test(pkg.trim()) && !!soName;

  // 目标 lib 目录预览（包名/ABI/设备变化时自动重查；只读）
  const {
    data: libDir,
    isFetching: previewing,
    isError: previewError,
    error: previewErr,
  } = useQuery({
    queryKey: ["pkg-lib-dir", deviceSerial, pkg.trim(), abi],
    queryFn: () => deviceApi.pkgLibDir(deviceSerial!, pkg.trim(), abi),
    enabled: canPreview,
    staleTime: 30_000,
    retry: false,
  });

  const pickSo = async () => {
    const picked = await pickFile({
      title: t("binary.so.pickTitle"),
      filters: [{ name: "Shared Object (*.so)", extensions: ["so"] }],
    });
    if (picked) {
      setLocalPath(picked);
      setResult(null);
    }
  };

  const fillForeground = async () => {
    if (!deviceSerial) return;
    try {
      const fg = await envApi.foreground(deviceSerial);
      if (fg.package) {
        setPkg(fg.package);
      } else {
        setNotice(fg.error ?? fg.hint ?? t("binary.so.noForeground"));
      }
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  const run = async () => {
    if (!deviceSerial || !localPath || !pkg.trim() || !soName.endsWith(".so")) return;
    setBusy(true);
    setResult(null);
    setNotice(null);
    try {
      setResult(await deviceApi.soReplace(deviceSerial, localPath, pkg.trim(), abi));
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setBusy(false);
    }
  };

  if (!adbReady) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
        {env?.hint ?? t("common.loading")}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto overflow-x-hidden">
      <p className="shrink-0 text-xs leading-relaxed text-muted-foreground">{t("binary.so.description")}</p>

      {online.length === 0 ? (
        <p className="shrink-0 text-xs text-muted-foreground">{t("adb.forward.rowInactive")}</p>
      ) : (
        <label className="flex shrink-0 items-center gap-2 text-xs">
          <span className="font-mono text-muted-foreground">-s</span>
          <select
            className="h-7 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
            value={deviceSerial ?? ""}
            onChange={(e) => {
              setDeviceSerial(e.target.value);
              setResult(null);
            }}
          >
            {online.map((d: DeviceEntry) => (
              <option key={d.serial} value={d.serial}>
                {d.model || d.serial}（{d.serial}）
              </option>
            ))}
          </select>
        </label>
      )}

      <div className="flex min-h-0 flex-col gap-3 rounded-lg border bg-card p-3 text-xs">
        {/* 本地 so 文件 */}
        <div className="flex items-center gap-2">
          <span className="w-16 shrink-0 text-muted-foreground">{t("binary.so.localFile")}</span>
          <input
            aria-label={t("binary.so.localFile")}
            title={t("binary.so.dragHint")}
            className="path-selectable h-7 min-w-0 flex-1 rounded-md border border-dashed border-input bg-transparent px-2 font-mono text-xs hover:border-foreground/40"
            placeholder={t("binary.so.dragPlaceholder")}
            value={localPath}
            onChange={(e) => setLocalPath(e.target.value)}
          />
          <Button size="sm" variant="outline" className="h-7 shrink-0 gap-1 px-2" onClick={() => void pickSo()}>
            <FolderInput className="h-3 w-3" />
            {t("binary.so.pick")}
          </Button>
        </div>

        {/* 包名 + 取前台 */}
        <div className="flex items-center gap-2">
          <span className="w-16 shrink-0 text-muted-foreground">{t("binary.so.pkg")}</span>
          <input
            aria-label={t("binary.so.pkg")}
            className="h-7 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
            placeholder="com.example.app"
            value={pkg}
            onChange={(e) => setPkg(e.target.value)}
          />
          <Button
            size="sm"
            variant="outline"
            className="h-7 shrink-0 px-2"
            disabled={!deviceSerial}
            onClick={() => void fillForeground()}
          >
            {t("binary.so.fromForeground")}
          </Button>
        </div>

        {/* ABI 选择 */}
        <div className="flex items-center gap-2">
          <span className="w-16 shrink-0 text-muted-foreground">{t("binary.so.abi")}</span>
          <div className="inline-flex rounded-lg bg-muted p-1" role="radiogroup" aria-label={t("binary.so.abi")}>
            {(["arm64", "arm"] as const).map((opt) => (
              <button
                key={opt}
                type="button"
                role="radio"
                aria-checked={abi === opt}
                onClick={() => setAbi(opt)}
                className={cn(
                  "rounded-md px-3 py-1 text-xs transition-colors",
                  abi === opt
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {opt === "arm64" ? t("binary.so.abi64") : t("binary.so.abi32")}
              </button>
            ))}
          </div>
        </div>

        {/* 目标预览 */}
        <div className="flex items-start gap-2">
          <span className="w-16 shrink-0 pt-1 text-muted-foreground">{t("binary.so.target")}</span>
          <div className="min-w-0 flex-1 rounded-md bg-muted/50 px-2 py-1 font-mono text-[11px] leading-relaxed break-all">
            {!canPreview ? (
              <span className="text-muted-foreground">{t("binary.so.targetIdle")}</span>
            ) : previewing ? (
              <span className="inline-flex items-center gap-1 text-muted-foreground">
                <RefreshCw className="h-3 w-3 animate-spin" />
                {t("common.loading")}
              </span>
            ) : previewError ? (
              <span className="text-destructive break-all">
                {String((previewErr as Error)?.message ?? previewErr)}
              </span>
            ) : (
              <span className="path-selectable">
                {libDir}
                <span className="text-emerald-500">/{soName || "?"}</span>
              </span>
            )}
          </div>
        </div>

        {/* root 提示 */}
        <p className="text-[11px] leading-relaxed text-muted-foreground">{t("binary.so.rootNote")}</p>

        <div className="flex justify-end">
          <Button
            size="sm"
            className="h-7 gap-1"
            disabled={busy || !deviceSerial || !canPreview || !soName.endsWith(".so") || !!previewError}
            onClick={() => void run()}
            data-testid="so-replace-run"
          >
            {busy ? <RefreshCw className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
            {busy ? t("binary.so.running") : t("binary.so.run")}
          </Button>
        </div>
      </div>

      {result && (
        <div
          className="shrink-0 rounded-md border px-2 py-1.5 text-xs"
          data-testid="so-replace-result"
        >
          <p
            className={
              result.verified
                ? "break-all font-mono text-emerald-600 dark:text-emerald-400"
                : "break-all font-mono text-destructive"
            }
          >
            {result.verified
              ? t("binary.so.success", { target: result.targetPath })
              : t("binary.so.notVerified", { target: result.targetPath })}
          </p>
          {result.rolledBack && (
            <p className="mt-1 text-amber-500" data-testid="so-replace-rolled-back">
              {t("binary.so.rolledBack")}
            </p>
          )}
          <ul className="mt-1 space-y-0.5">
            {result.steps.map((step, index) => (
              <li
                key={`${step.name}-${index}`}
                className="flex items-start gap-1.5 font-mono text-[11px] leading-relaxed"
                data-testid={`so-replace-step-${step.name}`}
              >
                <span className={step.ok ? "text-emerald-600" : "text-destructive"}>
                  {step.ok ? "\u2713" : "\u2715"}
                </span>
                <span className="shrink-0">{step.name}</span>
                {step.detail && (
                  <span className="min-w-0 break-all text-muted-foreground">{step.detail}</span>
                )}
              </li>
            ))}
          </ul>
          {result.backupPath && (
            <p className="mt-1 break-all text-[11px] text-muted-foreground">
              {t("binary.so.backupKept", { path: result.backupPath })}
            </p>
          )}
        </div>
      )}
      {notice && (
        <p className="shrink-0 break-all rounded-md border bg-muted/40 px-2 py-1 text-xs text-muted-foreground">
          {notice}
        </p>
      )}
    </div>
  );
}
