import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderOpen, Play, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { configApi } from "@/api/config";
import { hookApi, type JsFileDto } from "@/api/hook";
import { pickDirectory } from "@/api/dialog";
import { useI18n } from "@/i18n";

/** 配置键（与 Rust KEY_HOOK_WORKDIR 一致，进 ALLOWED_KEYS 白名单） */
export const KEY_HOOK_WORKDIR = "app.hook.workdir";

export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function formatMtime(sec: number): string {
  if (!sec) return "—";
  const d = new Date(sec * 1000);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/**
 * 区域 B · 脚本区（左下 2/9，§4）：工作目录（配置键持久化）+ 一层 *.js 列表。
 * 每项「启动」= 以当前 A 区设置 + 该脚本开会话；双击同义（托管页习惯一致）。
 * 不做编辑/新建/删除（非目标）。
 */
export function ScriptList({
  running,
  onStart,
}: {
  running: boolean;
  onStart: (script: JsFileDto) => void;
}) {
  const { t } = useI18n();
  const [workdir, setWorkdir] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void configApi
      .get(KEY_HOOK_WORKDIR, "")
      .then((v) => alive && setWorkdir(v.trim()))
      .catch(() => alive && setWorkdir(""));
    return () => {
      alive = false;
    };
  }, []);

  const {
    data: files,
    isFetching,
    error,
  } = useQuery({
    queryKey: ["hook", "jslist", workdir, tick],
    queryFn: () => hookApi.jsList(workdir ?? undefined),
    enabled: !!workdir,
    staleTime: 5_000,
    retry: false,
  });

  const pickDir = async () => {
    const picked = await pickDirectory(t("hook.frida.pickWorkdirTitle"));
    if (!picked) return;
    try {
      await configApi.set(KEY_HOOK_WORKDIR, picked);
      setWorkdir(picked);
      setNotice(null);
      setTick((n) => n + 1);
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  const items = useMemo(() => files ?? [], [files]);

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-lg border bg-card p-2.5 text-xs">
      <div className="flex shrink-0 items-center gap-2">
        <h3 className="font-medium">{t("hook.frida.scripts")}</h3>
        {workdir && (
          <Button
            size="sm"
            variant="ghost"
            className="h-6 gap-1 px-1.5 text-[11px]"
            onClick={() => void setTick((n) => n + 1)}
            disabled={isFetching}
          >
            <RefreshCw className={isFetching ? "h-3 w-3 animate-spin" : "h-3 w-3"} />
            {t("common.refresh")}
          </Button>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        <span className="text-[11px] text-muted-foreground">{t("hook.frida.workdir")}</span>
        <span
          className="path-selectable min-w-0 flex-1 truncate font-mono text-[11px]"
          title={workdir ?? ""}
        >
          {workdir || <span className="text-muted-foreground">{t("hook.frida.noWorkdir")}</span>}
        </span>
        <Button size="sm" variant="outline" className="h-6 shrink-0 gap-1 px-1.5 text-[11px]" onClick={() => void pickDir()}>
          <FolderOpen className="h-3 w-3" />
          {workdir ? t("hook.frida.changeDir") : t("hook.frida.pickDir")}
        </Button>
      </div>

      <div className="min-h-0 flex-1 space-y-1 overflow-y-auto overflow-x-hidden">
        {!workdir ? (
          <p className="pt-6 text-center text-[11px] leading-relaxed text-muted-foreground">
            {t("hook.frida.pickDirHint")}
          </p>
        ) : error ? (
          <p className="break-all rounded border border-destructive/50 bg-destructive/10 px-2 py-1 text-[11px] text-destructive">
            {String((error as Error)?.message ?? error)}
          </p>
        ) : items.length === 0 ? (
          <p className="pt-6 text-center text-[11px] text-muted-foreground">{t("hook.frida.empty")}</p>
        ) : (
          items.map((f) => (
            <div
              key={f.name}
              className="group flex cursor-pointer items-center gap-1.5 rounded border bg-transparent px-1.5 py-1 hover:bg-muted/40"
              onDoubleClick={() => !running && onStart(f)}
              title={t("hook.frida.startHint")}
            >
              <span className="path-selectable min-w-0 flex-1 truncate font-mono" title={f.path}>
                {f.name}
              </span>
              <span className="shrink-0 text-[10px] text-muted-foreground">{formatSize(f.size)}</span>
              <span className="hidden shrink-0 text-[10px] text-muted-foreground sm:inline">
                {formatMtime(f.mtime)}
              </span>
              <Button
                size="sm"
                variant="outline"
                className="h-5 shrink-0 gap-0.5 px-1 text-[10px] opacity-0 group-hover:opacity-100"
                disabled={running}
                onClick={(e) => {
                  e.stopPropagation();
                  onStart(f);
                }}
              >
                <Play className="h-2.5 w-2.5" />
                {t("hook.frida.launch")}
              </Button>
            </div>
          ))
        )}
      </div>
      {notice && (
        <p className="shrink-0 break-all text-[11px] text-destructive">{notice}</p>
      )}
    </section>
  );
}
