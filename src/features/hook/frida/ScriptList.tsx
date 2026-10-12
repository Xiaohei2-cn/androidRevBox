import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderOpen, Play, RefreshCw, ScrollText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PaneHeader } from "@/components/ui/pane-header";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { hookApi, type JsFileDto } from "@/api/hook";
import { useI18n } from "@/i18n";
import { pickAndPersistWorkdir } from "./workdir";

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
 * 区域 B · 脚本区（左下 2/9）：工作目录（配置键持久化，状态归 FridaPage 持有——
 * 右侧编辑器要拿同一个目录做读写）+ 一层 *.js 列表。
 *
 * 交互（第九十六轮，编辑器落地后）：
 * - **单击一行 = 在右侧编辑器里打开**（编辑是这一层的主线）；
 * - 行内「▶」= 以当前 A 区设置起官方 REPL 会话（双击不再启动——它与"单击打开"打架）。
 *
 * 显示口径不动（第九十二轮）：一行只有脚本名，大小 / 修改时间 / 完整路径收进 hover 气泡。
 */
export function ScriptList({
  workdir,
  onWorkdirChange,
  onStart,
  onOpen,
  refreshSignal,
  dirtyNames,
}: {
  /** 已配置的工作目录（"" = 未选择） */
  workdir: string | null;
  /** 用户选了新目录（config 已由调用方写入） */
  onWorkdirChange: (dir: string) => void;
  /** ▶：以当前设置起官方 REPL 会话（多开由终端自己的会话上限兜底） */
  onStart: (script: JsFileDto) => void;
  /** 单击一行：在编辑器打开 */
  onOpen: (script: JsFileDto) => void;
  /** 外部（编辑器新建/删除后）要求刷新列表：数值一变就重新拉取 */
  refreshSignal: number;
  /** 编辑器里处于未保存状态的文件名（行上标出来，别让她不知情地切走丢改动） */
  dirtyNames?: ReadonlySet<string>;
}) {
  const { t } = useI18n();
  const [tick, setTick] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);

  const {
    data: files,
    isFetching,
    error,
  } = useQuery({
    // refreshSignal 来自外面（编辑器新建/删除后推一步）：与本地「刷新」按钮合进同一个 key
    queryKey: ["hook", "jslist", workdir, tick, refreshSignal],
    queryFn: () => hookApi.jsList(workdir ?? undefined),
    enabled: !!workdir,
    staleTime: 5_000,
    retry: false,
  });

  const pickDir = async () => {
    // 与编辑器空态「选择目录」共用同一个实现：弹窗 → 落 config → 回报
    try {
      const picked = await pickAndPersistWorkdir(t("hook.frida.pickWorkdirTitle"));
      if (!picked) return;
      setNotice(null);
      onWorkdirChange(picked);
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  const items = useMemo(() => files ?? [], [files]);

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-xl border border-border/70 bg-card shadow-card p-2.5 text-xs">
      <div className="flex shrink-0 items-center gap-2">
        <PaneHeader compact icon={ScrollText} title={t("hook.frida.scripts")} className="min-w-0 flex-1" />
        {workdir && (
          <Button
            size="sm"
            variant="ghost"
            className="h-6 gap-1 px-1.5 text-11px"
            onClick={() => void setTick((n) => n + 1)}
            disabled={isFetching}
          >
            <RefreshCw className={isFetching ? "h-3 w-3 animate-spin" : "h-3 w-3"} />
            {t("common.refresh")}
          </Button>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        <span className="text-11px text-muted-foreground">{t("hook.frida.workdir")}</span>
        <span
          className="path-selectable min-w-0 flex-1 truncate font-mono text-11px"
          title={workdir ?? ""}
        >
          {workdir || <span className="text-muted-foreground">{t("hook.frida.noWorkdir")}</span>}
        </span>
        <Button size="sm" variant="outline" className="h-6 shrink-0 gap-1 px-1.5 text-11px" onClick={() => void pickDir()}>
          <FolderOpen className="h-3 w-3" />
          {workdir ? t("hook.frida.changeDir") : t("hook.frida.pickDir")}
        </Button>
      </div>

      <div className="pane-scroll min-h-0 flex-1 space-y-1 overflow-y-auto overflow-x-hidden">
        {!workdir ? (
          <p className="pt-6 text-center text-11px leading-relaxed text-muted-foreground">
            {t("hook.frida.pickDirHint")}
          </p>
        ) : error ? (
          <p className="break-all rounded border border-destructive/50 bg-destructive/10 px-2 py-1 text-11px text-destructive">
            {String((error as Error)?.message ?? error)}
          </p>
        ) : items.length === 0 ? (
          <p className="pt-6 text-center text-11px text-muted-foreground">{t("hook.frida.empty")}</p>
        ) : (
          /**
           * 行内只留脚本名（用户反馈：名字才是她要选的）。单击打开编辑器；
           * 「▶」（hover 出现）以当前设置启动会话——双击不再启动（与"单击打开"打架）。
           */
          items.map((f) => (
            <Tooltip key={f.name}>
              <TooltipTrigger asChild>
                <div
                  className="group flex cursor-pointer items-center gap-1.5 rounded border bg-transparent px-1.5 py-1 hover:bg-muted/40"
                  onClick={() => onOpen(f)}
                  data-testid={`script-row-${f.name}`}
                >
                  <span className="path-selectable line-clamp-2 min-w-0 flex-1 break-all font-mono leading-snug">
                    {f.name}
                    {dirtyNames?.has(f.name) && (
                      <span
                        className="pl-1 text-amber-500"
                        title={t("hook.frida.ide.unsaved")}
                        data-testid={`script-dirty-${f.name}`}
                      >
                        ●
                      </span>
                    )}
                  </span>
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-5 shrink-0 gap-0.5 px-1 text-10px opacity-0 group-hover:opacity-100"
                    onClick={(e) => {
                      e.stopPropagation();
                      onStart(f);
                    }}
                  >
                    <Play className="h-2.5 w-2.5" />
                    {t("hook.frida.launch")}
                  </Button>
                </div>
              </TooltipTrigger>
              <TooltipContent
                side="right"
                align="start"
                sideOffset={8}
                className="max-w-[min(26rem,55vw)] text-left"
                data-testid={`script-meta-${f.name}`}
              >
                <ScriptMeta file={f} />
              </TooltipContent>
            </Tooltip>
          ))
        )}
      </div>
      {notice && (
        <p className="shrink-0 break-all text-11px text-destructive">{notice}</p>
      )}
    </section>
  );
}

/**
 * hover 气泡的内容（从行里收回来的那几样）：完整名字 + 路径 + 大小 + 修改时间，
 * 末尾留一句点击语义。
 *
 * 单独成组件有两个原因：① 行与气泡共用一份 t()；② jsdom 没有排版，Radix 浮层一挂载
 * 就要 ~14 s（@floating-ui 在 jsdom 里把 getComputedStyle 当循环用），
 * 所以「气泡里到底写了什么」必须能绕开浮层直接渲染来测——那是可信的判据，
 * 「气泡会不会弹」归真机手工矩阵（docs/frida_terminal_phases.md §5）。
 */
export function ScriptMeta({ file }: { file: JsFileDto }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-1">
      <p className="break-all font-mono">{file.name}</p>
      <dl className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5 text-10px text-muted-foreground">
        <dt>{t("hook.frida.metaPath")}</dt>
        <dd className="break-all font-mono">{file.path}</dd>
        <dt>{t("hook.frida.metaSize")}</dt>
        <dd className="tabular-nums">{formatSize(file.size)}</dd>
        <dt>{t("hook.frida.metaMtime")}</dt>
        <dd className="tabular-nums">{formatMtime(file.mtime)}</dd>
      </dl>
      <p className="text-10px text-muted-foreground">{t("hook.frida.startHint")}</p>
    </div>
  );
}