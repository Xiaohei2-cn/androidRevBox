import { memo, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Plus, Search, X } from "lucide-react";

import type { ChefOperationDto } from "@/api/chef";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardHeader, CardSubtitle, CardTitle } from "@/components/ui/card";
import { IconTile } from "@/components/ui/icon-tile";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { groupStyle } from "./catalogueGroups";
import { operationHint, searchHaystack, type ChefLabels } from "./labels";

interface CatalogueProps {
  operations: ChefOperationDto[];
  labels: ChefLabels;
  /** 配方里每个算法用了几次（清单上标出来，组合时不用回头数） */
  usedCounts: Map<string, number>;
  /** 点行尾的加号：追加到配方末尾。条目本体不再负责点击（见条目上的注释） */
  onAdd: (id: string) => void;
  /** 在条目本体上按下：开始指针拖拽，松手落在配方区就插到那个位置 */
  onPickStart: (id: string, event: React.PointerEvent) => void;
  loading: boolean;
  loadError: string | null;
  className?: string;
}

/**
 * 最左侧的算法清单：**默认全部折叠**，按卡片语言排版。
 *
 * 两条选入路径分工明确，不做重复：
 * - 行尾的**加号**（一直可见）= 点击选入，追加到配方末尾；
 * - **条目本体按下并拖动** = 指针拖拽选入，松手处就是插入位置。
 * 之前整行都是按钮，点哪儿都会加一步，误触得去配方里删——加号既然画出来了就该由它负责。
 * 键盘路径走加号：它是真按钮，Tab 得到、Enter/Space 生效。
 *
 * 名称/分类/参数名都走词典，所以设置里换语言后这里立刻跟着换。
 * 组件包 memo：执行结果回来时不该重画这 120 行（见 labels.ts 与闪烁那一节）。
 */
export const Catalogue = memo(function Catalogue({
  operations,
  labels,
  usedCounts,
  onAdd,
  onPickStart,
  loading,
  loadError,
  className,
}: CatalogueProps) {
  const [query, setQuery] = useState("");
  /** 空集合 = 全部折叠（默认态） */
  const [open, setOpen] = useState<Set<string>>(new Set());
  /** 提示语要带参数名，这里按 id 取回完整的参数表 */
  const byId = useMemo(
    () => new Map(operations.map((operation) => [operation.id, operation])),
    [operations],
  );

  const needle = query.trim().toLocaleLowerCase("en-US");
  const groups = useMemo(() => {
    const byCategory = new Map<string, string[]>();
    for (const operation of operations) {
      const words = searchHaystack([
        operation.id,
        labels.operation(operation.id),
        labels.description(operation.id),
        labels.category(operation.category),
        // 参数名与候选值也进搜索，并且**原词 + 当前语言的译名都给**：
        // 搜 pkcs7 要命中带 padding 选项的算法，中文界面搜「填充」同样要命中。
        ...operation.args.flatMap((arg) => [
          arg.name,
          labels.argument(arg.name),
          ...arg.options,
          ...arg.options.map((option) => labels.option(option)),
        ]),
      ]);
      if (needle && !words.includes(needle)) continue;
      byCategory.set(operation.category, [
        ...(byCategory.get(operation.category) ?? []),
        operation.id,
      ]);
    }
    return [...byCategory.entries()];
    // labels 随语言变，必须进依赖：否则切语言后清单还是旧文案
  }, [operations, needle, labels]);

  const total = groups.reduce((sum, [, items]) => sum + items.length, 0);
  const searching = needle.length > 0;
  const allOpen = groups.length > 0 && groups.every(([category]) => open.has(category));

  return (
    <Card
      data-testid="chef-pane-catalogue"
      aria-label={labels.ui("catalogueAria")}
      className={cn("flex min-h-0 shrink-0 flex-col overflow-hidden", className)}
    >
      <CardHeader className="flex-col items-stretch gap-2 pb-2.5">
        <div className="flex items-center gap-2">
          <IconTile icon={Search} toneClass="bg-primary/10 text-primary" />
          <div className="min-w-0 flex-1">
            <CardTitle className="text-xs">{labels.ui("catalogueAria")}</CardTitle>
            <CardSubtitle className="mt-1">{labels.ui("catalogueHint")}</CardSubtitle>
          </div>
          <Badge variant="outline" className="tabular-nums">
            {total}
          </Badge>
        </div>
        <div className="relative min-w-0">
          {/* 定位用 inset-y-0 + flex，不用 -translate-y-1/2：transform 会在透明窗口里
              给图标单独立合成层，重绘那一帧整窗闪（globals.css 第三轮闪烁修复） */}
          <span className="pointer-events-none absolute inset-y-0 left-2.5 flex items-center text-muted-foreground">
            <Search className="h-3.5 w-3.5" />
          </span>
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={labels.ui("searchPlaceholder")}
            aria-label={labels.ui("searchAria")}
            className="h-7 rounded-lg pl-8 pr-7 text-xs"
          />
          {query && (
            <button
              type="button"
              onClick={() => setQuery("")}
              aria-label={labels.ui("searchClear")}
              className="absolute inset-y-0 right-1 flex items-center rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <X className="h-3 w-3" />
            </button>
          )}
        </div>
        {!searching && groups.length > 0 && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 w-full justify-center rounded-md text-[11px] text-muted-foreground"
            onClick={() =>
              setOpen(allOpen ? new Set() : new Set(groups.map(([category]) => category)))
            }
          >
            {allOpen ? labels.ui("collapseAll") : labels.ui("expandAll")}
          </Button>
        )}
      </CardHeader>

      <div className="pane-scroll min-h-0 flex-1 overflow-y-auto px-2.5 pb-2.5">
        {loading && (
          <div className="flex flex-col gap-1.5 pt-1.5">
            <p className="px-0.5 text-[11px] text-muted-foreground">{labels.ui("loading")}</p>
            {[0, 1, 2, 3, 4].map((row) => (
              <div key={row} className="h-8 rounded-lg bg-muted/60" />
            ))}
          </div>
        )}
        {loadError && (
          <p className="px-1 py-4 text-xs leading-relaxed text-destructive">{loadError}</p>
        )}
        {!loading && !loadError && total === 0 && (
          <p className="px-1 py-6 text-center text-xs text-muted-foreground">
            {labels.ui("searchEmpty")}
          </p>
        )}
        {groups.map(([category, ids]) => {
          const expanded = searching || open.has(category);
          const style = groupStyle(category);
          return (
            <section key={category} className="mt-1 first:mt-1.5">
              <button
                type="button"
                onClick={() =>
                  setOpen((previous) => {
                    const next = new Set(previous);
                    if (next.has(category)) next.delete(category);
                    else next.add(category);
                    return next;
                  })
                }
                aria-expanded={expanded}
                className={cn(
                  "flex w-full items-center gap-2 rounded-xl px-2 py-1.5 text-left transition-colors",
                  "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                  expanded ? "bg-muted/60" : "hover:bg-muted/40",
                )}
              >
                <IconTile icon={style.icon} toneClass={style.toneClass} size="sm" />
                <span className="min-w-0 flex-1 truncate text-xs font-medium">
                  {labels.category(category)}
                </span>
                <Badge variant={expanded ? "primary" : "muted"} className="tabular-nums">
                  {ids.length}
                </Badge>
                {expanded ? (
                  <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                )}
              </button>
              {expanded && (
                <ul className="pane-inset mt-1 flex flex-col gap-px p-1">
                  {ids.map((id) => {
                    const used = usedCounts.get(id) ?? 0;
                    const addLabel = labels.ui("addAria", { name: labels.operation(id) });
                    return (
                      <li key={id}>
                        {/*
                          条目本体：按下可拖（松手处即插入位置），**单击 = 直接追加到配方末尾**。
                          两个入口都留着：条目本体负责"就近选一个"，行尾加号负责明示"这里是点击位"。

                          三件事必须同时做，拖拽才真的动（第一版就是漏了前两件，表现是
                          "按下有反馈、一动就没反应"）：
                          1. `select-none`：全局是 `user-select: text`（信息文本默认可选，P9），
                             一按就起文本选区，WebKit 会把这次手势转成**原生文本拖拽**，
                             pointermove/pointerup 从此不再交给我们，落点永远算不出来；
                          2. `draggable={false}` + dragstart 阻断：连原生拖拽的入口一起堵掉，
                             顺手防止 WKWebView 里"把选中文字拖出去"的默认行为；
                          3. `setPointerCapture`：把这次指针的后续事件锁在这个元素上，
                             即便系统仍想抢手势，move/up 也一定回到我们手里。
                        */}
                        <div
                          draggable={false}
                          onDragStart={(event) => event.preventDefault()}
                          onPointerDown={(event) => onPickStart(id, event)}
                          onClick={() => onAdd(id)}
                          title={operationHint(labels, byId.get(id) ?? { id, args: [] })}
                          className={cn(
                            "group flex w-full cursor-grab touch-none select-none items-center gap-1.5 rounded-lg px-2 py-1.5",
                            "text-left text-xs transition-colors hover:bg-background",
                            used > 0 && "text-primary",
                          )}
                        >
                          <span className="min-w-0 flex-1 truncate">{labels.operation(id)}</span>
                          {used > 0 && (
                            <Badge variant="primary" title={labels.ui("usedTimes", { n: used })}>
                              x{used}
                            </Badge>
                          )}
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-5 w-5 shrink-0 rounded-md text-muted-foreground hover:bg-primary/10 hover:text-primary"
                            onClick={(event) => {
                              // 加号在条目里，不挡住冒泡的话一次点击会加两步
                              event.stopPropagation();
                              onAdd(id);
                            }}
                            aria-label={addLabel}
                            title={addLabel}
                          >
                            <Plus className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </li>                    );
                  })}
                </ul>
              )}
            </section>
          );
        })}
      </div>
    </Card>
  );
});
