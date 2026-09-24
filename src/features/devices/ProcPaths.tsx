import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, ChevronRight, Copy, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { deviceApi, type ProcReadResult } from "@/api/device";
import type { ProcPath } from "@/api/env";
import { useI18n } from "@/i18n";
import { PathText } from "@/components/ui/PathText";
import { cn } from "@/lib/utils";

/**
 * `/proc/<pid>` 关键路径这一栏（设备信息页）。
 *
 * 分成"读到的摘要"和"要点才读"两类，是这个组件存在的理由：
 * `maps` 属于**别的用户的进程**，Agent 以 shell 身份读它一定 `EACCES`（Android 14 实测，
 * 只有 root 读得到）。以前每次刷新都替它跑一次 `wc -l`，既白跑，又在界面上留一句
 * 红色的"不可读"——看着像工具坏了，其实是权限边界，而且用户想看的是内容不是那个字。
 * 现在这一项标成 `readOnDemand`，界面上只给一个箭头：不点不发请求，点了才跳详情去读。
 */
const READ_MAX_LINES = 400;

type DetailTarget = { serial: string; pid: number; file: "maps" | "cmdline" | "status" };

export function ProcPaths({
  serial,
  pid,
  entries,
}: {
  serial: string;
  /** 前台应用的 pid（字符串形态来自设备信息 DTO） */
  pid?: string | null;
  entries: ProcPath[];
}) {
  const { t } = useI18n();
  const [detail, setDetail] = useState<DetailTarget | null>(null);
  const pidNumber = Number(pid ?? "");

  if (detail && Number.isFinite(pidNumber) && pidNumber > 0) {
    return (
      <ProcFileDetail
        target={detail}
        onBack={() => setDetail(null)}
        onReload={() => setDetail({ ...detail })}
      />
    );
  }

  return (
    <ul className="space-y-1" data-testid={`fg-proc-paths-${serial}`}>
      {entries.length === 0 && (
        <li className="text-muted-foreground">{t("dashboard.foreground.noProc")}</li>
      )}
      {entries.map((entry) => {
        const file = entry.name as "maps" | "cmdline" | "status";
        const openable = file === "maps" || file === "cmdline" || file === "status";
        return (
          <li key={entry.path} className="flex items-baseline gap-2 font-mono">
            <PathText value={entry.path} className="shrink-0 text-foreground" />
            {entry.readOnDemand ? (
              // 「没读」不等于「读不到」：这里不给红字，只给入口
              <span className="min-w-0 flex-1 break-all text-muted-foreground">
                {t("dashboard.foreground.readOnDemand")}
              </span>
            ) : (
              <span
                className={cn(
                  "min-w-0 flex-1 break-all",
                  entry.readable ? "text-muted-foreground" : "text-amber-500",
                )}
                title={entry.summary ?? undefined}
              >
                {entry.readable ? entry.summary || t("common.emptyValue") : t("dashboard.foreground.unreadable")}
              </span>
            )}
            {openable && (
              <Button
                size="sm"
                variant="ghost"
                type="button"
                className="h-6 shrink-0 gap-1 px-1.5 text-[11px]"
                data-testid={`fg-proc-open-${serial}-${file}`}
                aria-label={`${t("dashboard.foreground.viewDetail")} ${entry.path}`}
                disabled={!Number.isFinite(pidNumber) || pidNumber <= 0}
                onClick={() => setDetail({ serial, pid: pidNumber, file })}
              >
                {/* 入口必须看得见文字：只放一个图标就是上一轮"悬停才显示 = 用户以为
                    没这个功能"的同一个坑，只是换了个藏法 */}
                <ChevronRight className="h-3.5 w-3.5" />
                {t("dashboard.foreground.viewDetail")}
              </Button>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function ProcFileDetail({
  target,
  onBack,
  onReload,
}: {
  target: DetailTarget;
  onBack: () => void;
  onReload: () => void;
}) {
  const { t } = useI18n();
  const [nonce, setNonce] = useState(0);
  const query = useQuery<ProcReadResult>({
    // 只在点开之后才发：queryKey 里有 file，切换文件不会复用旧内容
    queryKey: ["device", "proc-read", target.serial, target.pid, target.file, nonce],
    queryFn: () =>
      deviceApi.procRead(target.serial, target.pid, target.file, READ_MAX_LINES),
    enabled: true,
    retry: false,
    staleTime: 2_000,
  });

  const copy = () => {
    if (query.data) void navigator.clipboard?.writeText(query.data.text);
  };

  return (
    <div
      className="rounded-lg border bg-card p-2"
      data-testid={`fg-proc-detail-${target.serial}-${target.file}`}
    >
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 gap-1 px-1.5 text-[11px]"
          data-testid="fg-proc-detail-back"
          onClick={onBack}
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {t("dashboard.foreground.detailBack")}
        </Button>
        <PathText
          value={query.data?.path ?? `/proc/${target.pid}/${target.file}`}
          className="min-w-0 flex-1 font-mono text-[11px]"
        />
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 px-1.5"
          aria-label={t("common.copy")}
          disabled={!query.data}
          onClick={copy}
        >
          <Copy className="h-3.5 w-3.5" />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="h-6 px-1.5"
          aria-label={t("common.refresh")}
          disabled={query.isFetching}
          onClick={() => {
            setNonce((n) => n + 1);
            onReload();
          }}
        >
          <RefreshCw className={cn("h-3.5 w-3.5", query.isFetching && "animate-spin")} />
        </Button>
      </div>
      <p className="mt-1 text-[11px] text-muted-foreground" data-testid="fg-proc-detail-meta">
        {query.isFetching && !query.data
          ? t("dashboard.foreground.detailLoading")
          : query.data
            ? detailMeta(t, query.data)
            : query.isError
              ? `${t("dashboard.foreground.detailFailed")}：${String(
                  (query.error as Error)?.message ?? query.error,
                )}`
              : t("common.none")}
      </p>
      {query.data ? (
        <pre
          className="mt-1 max-h-64 overflow-auto rounded bg-muted/40 p-1.5 font-mono text-[10.5px] leading-snug"
          data-testid="fg-proc-detail-body"
        >
          {query.data.text || t("common.emptyValue")}
        </pre>
      ) : null}
    </div>
  );
}

function detailMeta(t: (key: string, values?: Record<string, string | number>) => string, data: ProcReadResult) {
  const via =
    data.read_via === "root"
      ? t("dashboard.foreground.detailViaRoot")
      : t("dashboard.foreground.detailViaShell");
  // 三件事都要看得见：读到多少、有没有被截断、这次是谁的权限
  return data.truncated
    ? t("dashboard.foreground.detailLinesTruncated", {
        total: data.total_lines,
        shown: data.returned_lines,
        via,
      })
    : t("dashboard.foreground.detailLines", {
        total: data.total_lines,
        shown: data.returned_lines,
        via,
      });
}
