import { useState } from "react";
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  CornerDownRight,
  Power,
  Trash2,
} from "lucide-react";

import type { ChefOperationDto, ChefStepResult } from "@/api/chef";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { StatusBadge } from "@/components/ui/status-badge";
import { cn } from "@/lib/utils";
import { groupStyle } from "./catalogueGroups";
import type { ChefLabels } from "./labels";
import { StepArgs } from "./StepArgs";
import {
  chipValue,
  moveBy,
  nonDefaultArgs,
  type RecipeStep,
} from "./recipe";

interface RecipeProps {
  steps: RecipeStep[];
  operations: Map<string, ChefOperationDto>;
  labels: ChefLabels;
  /** 已执行的结果（按可见步骤编号）；未执行时 undefined */
  results: ChefStepResult[] | undefined;
  /** 失败那一步的 key（结果按可见步骤编号，换算成 key 才对得上） */
  failedKey: string | null;
  /** 正在从清单往这里拖的算法 id（null = 没在拖） */
  pickingId: string | null;
  /** 落点下标；null = 指针不在本面板内 */
  dropIndex: number | null;
  /** 把面板根节点交给拖拽 hook 做落点几何判定 */
  setArea: (el: HTMLElement | null) => void;
  emptyTitle: string;
  emptyHint: string;
  noArgsHint: string;
  onChange: (steps: RecipeStep[]) => void;
  onRemoveAt: (index: number) => void;
}

/**
 * 算法组合的列表本体（面板外壳与运行控件在 Workbench 的卡片头里）。
 *
 * 每一步是一张可折叠的小卡：左侧一条**导轨**串起序号，表示字节从上往下流；
 * 卡片左侧那道分类色带让人不看字也知道这一步是哪类算法；
 * 折叠态直接列出**被改过的参数**（等于默认值的不占位）；
 * 右侧状态位是固定槽位（未执行/待运行/字节数/失败），结果回来不会推动左边的内容。
 */
export function Recipe({
  steps,
  operations,
  labels,
  results,
  failedKey,
  pickingId,
  dropIndex,
  setArea,
  emptyTitle,
  emptyHint,
  noArgsHint,
  onChange,
  onRemoveAt,
}: RecipeProps) {
  /** 哪些步骤展开了参数区（拖拽态不影响它） */
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  // 被停用的步骤不参与执行，编号要和 results 对齐就得跳过它们
  let visible = -1;
  const perStep = steps.map((step) => {
    if (step.disabled) return null;
    visible += 1;
    return results?.[visible] ?? null;
  });

  return (
    <div
      ref={setArea}
      data-testid="chef-recipe-area"
      data-picking={pickingId ?? ""}
      className={cn(
        "pane-scroll min-h-0 flex-1 overflow-y-auto rounded-xl px-2.5 pb-2.5 transition-colors",
        // 一进入拖拽态就亮起来（不等到落点算出来）：用户要的是"我拖着东西时它知道我来了"
        pickingId !== null && "bg-primary/5 ring-1 ring-inset ring-primary/40",
      )}
    >      {steps.length === 0 ? (
        <div
          className={cn(
            "flex h-full min-h-0 items-center justify-center rounded-xl p-1",
            // 空配方是"拖进来"最常见的第一下：一进入拖拽态就描边 + 淡底，别等落点
            pickingId !== null && "bg-primary/5 ring-1 ring-inset ring-primary/40",
          )}
        >
          <EmptyState
            icon={<CornerDownRight className="h-4 w-4" />}
            title={emptyTitle}
            description={emptyHint}
            className="h-full min-h-0 border-0 bg-transparent py-0"
          />
        </div>
      ) : (
        <ul className="space-y-1">
          {steps.map((step, index) => {
            const operation = operations.get(step.id);
            if (!operation) return null;
            const result = perStep[index];
            const open = expanded[step.key] ?? false;
            const failed = Boolean(result && !result.ok);
            const style = groupStyle(operation.category);
            return (
              <li
                key={step.key}
                data-testid="chef-step"
                data-failed={failed ? "true" : undefined}
                className={cn(
                  // 与全站列表同一刻度：rounded-xl + 极淡外阴影（这里不用 ListRow：
                  // 算法中心有独立的防闪烁闸门，且这些行支持拖放排序，结构不能换）
                  "overflow-hidden rounded-xl border bg-card/70 shadow-row transition-colors",
                  failed
                    ? "border-destructive/60 bg-destructive/5"
                    : "border-border/70 hover:border-primary/40",
                  step.disabled && "bg-muted/30 opacity-70",
                  dropIndex === index && "border-primary",
                )}
                data-step-index={index}
              >
                {pickingId !== null && dropIndex === index && (
                  <p className="border-b border-dashed border-primary/60 bg-primary/5 px-3 py-0.5 text-[10px] font-medium text-primary">
                    {labels.ui("dropHere")}
                  </p>
                )}
                <div className="flex items-center gap-2 px-1.5 py-1.5">
                  {/* 导轨：序号 + 左侧那道分类色带 */}
                  <span
                    className={cn(
                      "flex h-5 w-5 shrink-0 items-center justify-center rounded-md text-[10px] font-semibold tabular-nums",
                      failed
                        ? "bg-destructive/15 text-destructive"
                        : step.disabled
                          ? "bg-muted text-muted-foreground"
                          : "bg-primary/10 text-primary",
                    )}
                  >
                    {index + 1}
                  </span>
                  <span
                    className={cn("h-6 w-0.5 shrink-0 rounded-full", style.toneClass.split(" ")[0])}
                    aria-hidden
                  />
                  <button
                    type="button"
                    onClick={() =>
                      setExpanded((previous) => ({ ...previous, [step.key]: !open }))
                    }
                    aria-expanded={open}
                    aria-label={labels.operation(step.id)}
                    className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md py-0.5 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  >
                    {open ? (
                      <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    )}
                    <span className="shrink-0 text-xs font-medium">
                      {labels.operation(step.id)}
                    </span>
                    {!open && (
                      <ArgChips
                        step={step}
                        operation={operation}
                        labels={labels}
                        defaultLabel={labels.ui("defaultArgs")}
                      />
                    )}
                  </button>
                  <StepStatus
                    result={result}
                    disabled={step.disabled}
                    labels={labels}
                    failed={failed}
                  />
                  <StepButtons
                    index={index}
                    total={steps.length}
                    disabled={step.disabled}
                    labels={labels}
                    onMove={(delta) => onChange(moveBy(steps, index, delta))}
                    onToggleDisabled={() =>
                      onChange(
                        steps.map((item, position) =>
                          position === index ? { ...item, disabled: !item.disabled } : item,
                        ),
                      )
                    }
                    onRemove={() => onRemoveAt(index)}
                  />
                </div>
                {(open || failed) && (
                  <div className="pane-inset mx-1.5 mb-1.5 p-2.5">
                    <StepArgs
                      operation={operation}
                      args={step.args}
                      labels={labels}
                      inertLabel={labels.ui("inertField")}
                      placeholder={noArgsHint}
                      inertFields={inertFields(step, operation)}
                      onChange={(name, value) =>
                        onChange(
                          steps.map((item, position) =>
                            position === index
                              ? { ...item, args: { ...item.args, [name]: value } }
                              : item,
                          ),
                        )
                      }
                    />
                  </div>
                )}
                {result && !result.ok && result.error && (
                  <p
                    role="alert"
                    className="flex items-start gap-1.5 border-t border-destructive/40 px-2.5 py-1.5 text-[11px] leading-snug text-destructive"
                  >
                    <AlertCircle className="mt-0.5 h-3 w-3 shrink-0" />
                    <span className="min-w-0 break-words">{result.error}</span>
                  </p>
                )}
                {result?.ok && !result.utf8 && (
                  <p className="border-t border-border/60 px-2.5 py-1.5 text-[11px] leading-snug text-muted-foreground">
                    {labels.ui("binaryHint")}
                    {result.preview ? ` ${result.preview.slice(0, 40)}` : ""}
                  </p>
                )}
              </li>
            );
          })}
          {pickingId !== null && dropIndex === steps.length && (
            <li className="rounded-xl border border-dashed border-primary/60 bg-primary/5 px-3 py-1 text-[10px] font-medium text-primary">
              {labels.ui("dropHere")}
            </li>
          )}
        </ul>
      )}
      {failedKey && (
        <p className="mt-1.5 rounded-lg bg-destructive/10 px-2.5 py-1.5 text-[11px] text-destructive">
          {labels.ui("stoppedAt", {
            name: labels.operation(steps.find((item) => item.key === failedKey)?.id ?? ""),
          })}
        </p>
      )}
    </div>
  );
}

/** 每步右侧的状态槽（固定宽度，见下面 w-[3.75rem]） */
function StepStatus({
  result,
  disabled,
  failed,
  labels,
}: {
  result: ChefStepResult | null;
  disabled: boolean;
  failed: boolean;
  labels: ChefLabels;
}) {
  // 固定槽位：有结果/没结果占同一块地方，第一次跑完不会把左边的名字推一下
  return (
    <span className="flex w-[3.75rem] shrink-0 justify-end">
      {disabled ? (
        <Badge variant="muted">{labels.ui("step.disable")}</Badge>
      ) : failed ? (
        <StatusBadge tone="error">
          <AlertCircle className="h-3 w-3" />
        </StatusBadge>
      ) : result?.ok ? (
        <StatusBadge tone="ok">{`${result.bytes} B`}</StatusBadge>
      ) : null}
    </span>
  );
}

/**
 * 折叠行上的参数摘要。
 * 规则：只显示**与默认值不同**的参数（一条 AES 链 8 个参数有 6 个没动过，全列等于没列）；
 * 布尔参数为真时就写参数名本身（"不换行" 比 "noWrap=true" 顺）；值太长截断。
 */
function ArgChips({
  step,
  operation,
  labels,
  defaultLabel,
}: {
  step: RecipeStep;
  operation: ChefOperationDto;
  labels: ChefLabels;
  defaultLabel: string;
}) {
  const changed = nonDefaultArgs(step, operation);
  if (changed.length === 0) {
    return (
      <span className="min-w-0 truncate text-[10px] text-muted-foreground">{defaultLabel}</span>
    );
  }
  return (
    <span className="flex min-w-0 items-center gap-1 overflow-hidden">
      {changed.slice(0, 3).map((entry) => (
        <Badge key={entry.name} variant="mono" className="max-w-[7rem] truncate">
          {entry.kind === "bool"
            ? labels.argument(entry.name)
            : `${labels.argument(entry.name)}=${chipValue(entry, labels.option)}`}
        </Badge>
      ))}
      {changed.length > 3 && (
        <Badge variant="outline" className="tabular-nums">
          +{changed.length - 3}
        </Badge>
      )}
    </span>
  );
}

function StepButtons({
  index,
  total,
  disabled,
  labels,
  onMove,
  onToggleDisabled,
  onRemove,
}: {
  index: number;
  total: number;
  disabled: boolean;
  labels: ChefLabels;
  onMove: (delta: number) => void;
  onToggleDisabled: () => void;
  onRemove: () => void;
}) {
  const title = (key: string) => labels.ui(`step.${key}`);
  return (
    <span className="flex shrink-0 items-center gap-px rounded-lg bg-muted/60 p-0.5">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-5 w-5 rounded-md text-muted-foreground hover:text-foreground"
        disabled={index === 0}
        onClick={() => onMove(-1)}
        aria-label={title("moveUp")}
      >
        <ArrowUp className="h-3 w-3" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-5 w-5 rounded-md text-muted-foreground hover:text-foreground"
        disabled={index === total - 1}
        onClick={() => onMove(1)}
        aria-label={title("moveDown")}
      >
        <ArrowDown className="h-3 w-3" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className={cn(
          "h-5 w-5 rounded-md",
          disabled ? "text-warning" : "text-muted-foreground hover:text-foreground",
        )}
        onClick={onToggleDisabled}
        aria-pressed={disabled}
        aria-label={title(disabled ? "enable" : "disable")}
      >
        <Power className="h-3 w-3" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-5 w-5 rounded-md text-muted-foreground hover:text-destructive"
        onClick={onRemove}
        aria-label={title("remove")}
      >
        <Trash2 className="h-3 w-3" />
      </Button>
    </span>
  );
}

/** 参数在当前模式下根本不被读的时候，别装作它有效 */
function inertFields(step: RecipeStep, operation: ChefOperationDto): string[] {
  const mode = step.args.mode ?? "";
  const inert: string[] = [];
  if (mode === "ecb" && operation.args.some((arg) => arg.name === "iv")) inert.push("iv", "ivFormat");
  if (["ctr", "cfb8", "cfb128", "ofb", "gcm"].includes(mode)) inert.push("padding");
  return inert;
}
