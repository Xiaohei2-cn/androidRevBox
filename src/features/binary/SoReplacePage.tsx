import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderInput, Play, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type DeviceEntry } from "@/api/device";
import { envApi } from "@/api/env";
import { pickFile } from "@/api/dialog";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * so 替换（二进制主标签子页）：修补后的 .so 直接写回安装目录，免重打包。
 * 流程：① push 本地 so 到 /data/local/tmp；② dumpsys package 查安装 lib 目录，
 * 按所选 ABI（arm64=64位 / arm=32位）拼目标路径，su -c 'cat tmp > 目标' 覆写；
 * ③ 删临时文件。目标路径实时预览（只读查询），执行前需停止目标应用
 * （运行中的 so 覆写会 text file busy）。
 */

type Abi = "arm64" | "arm";

export function SoReplacePage() {
  const { t } = useI18n();
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [localPath, setLocalPath] = useState("");
  const [pkg, setPkg] = useState("");
  const [abi, setAbi] = useState<Abi>("arm64");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
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
      const target = await deviceApi.soReplace(deviceSerial, localPath, pkg.trim(), abi);
      setResult(target);
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
            className="path-selectable h-7 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
            placeholder="/path/to/patched/libxxx.so"
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
        <p className="shrink-0 break-all rounded-md border border-emerald-500/50 bg-emerald-500/10 px-2 py-1 font-mono text-xs text-emerald-600 dark:text-emerald-400" data-testid="so-replace-result">
          {t("binary.so.success", { target: result })}
        </p>
      )}
      {notice && (
        <p className="shrink-0 break-all rounded-md border bg-muted/40 px-2 py-1 text-xs text-muted-foreground">
          {notice}
        </p>
      )}
    </div>
  );
}
