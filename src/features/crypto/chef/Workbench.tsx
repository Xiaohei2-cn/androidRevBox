import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Braces,
  Check,
  ClipboardCopy,
  Play,
  Save,
  ScanText,
  TextCursorInput,
  Trash2,
  Workflow,
  X,
  Zap,
} from "lucide-react";
import { useMutation, useQuery } from "@tanstack/react-query";

import { chefApi, INPUT_ENCODINGS, OUTPUT_DISPLAYS, type ChefBakeResult } from "@/api/chef";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardSubtitle,
  CardTitle,
} from "@/components/ui/card";
import { IconTile } from "@/components/ui/icon-tile";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import { Catalogue } from "./Catalogue";
import { usePickDrag } from "./usePickDrag";
import { useChefLabels } from "./labels";
import { Recipe } from "./RecipePanel";
import {
  hydrateSteps,
  insertAt,
  loadSaved,
  loadWorkbench,
  operationsById,
  parseRecipe,
  removeAt,
  saveSaved,
  saveWorkbench,
  serializeRecipe,
  stepFromOperation,
  toBakeRequest,
  visibleIndexOf,
  type RecipeStep,
  type SavedRecipe,
} from "./recipe";

/**
 * 自动执行的防抖间隔。
 * 250ms 太急：一路打字会把整条链连跑十几遍，输出区反复重画；
 * 透明窗口里"重画"是能看见的（第三轮闪烁修复说的就是这个）。
 * 350ms 是"手打停下来就出结果"和"不打架"之间的折中。
 */
const AUTO_FIRE_DELAY = 350;
const PERSIST_DELAY = 400;

/**
 * 算法合集（CyberChef 网页版的三栏布局）：
 * 左 = 算法清单，中 = 算法组合（拖进来排队），右 = 上输入 / 下输出。
 */
export function Workbench() {
  const { t } = useI18n();
  const labels = useChefLabels();
  const operationsQuery = useQuery({
    queryKey: ["chef", "operations"],
    queryFn: chefApi.operations,
    staleTime: Infinity,
  });
  // 注意要包装：`?? []` 每次渲染都会给出新数组，useMemo/useEffect 的依赖会失效
  const operations = useMemo(() => operationsQuery.data ?? [], [operationsQuery.data]);
  const byId = useMemo(() => operationsById(operations), [operations]);

  const [steps, setSteps] = useState<RecipeStep[]>([]);
  const [input, setInput] = useState("");
  const [inputEncoding, setInputEncoding] = useState("utf8");
  const [display, setDisplay] = useState<string>("text");
  const [autoFire, setAutoFire] = useState(true);
  const [restored, setRestored] = useState(false);
  const [saved, setSaved] = useState<SavedRecipe[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const bake = useMutation({ mutationFn: chefApi.bake });
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fireTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 清单到位后才回填上次的配方：参数默认值来自清单，早回填会把参数丢掉
  useEffect(() => {
    if (restored || operations.length === 0) return;
    const stored = loadWorkbench();
    if (stored) {
      setSteps(hydrateSteps(stored.steps, byId));
      setInput(stored.input);
      setInputEncoding(stored.inputEncoding);
      setDisplay(stored.display);
      setAutoFire(stored.autoFire);
    }
    setSaved(loadSaved());
    setRestored(true);
  }, [operations, restored, byId]);

  useEffect(() => {
    if (!restored) return;
    if (persistTimer.current) clearTimeout(persistTimer.current);
    persistTimer.current = setTimeout(() => {
      saveWorkbench({
        input,
        inputEncoding,
        autoFire,
        display,
        steps: steps.map((step) => ({
          id: step.id,
          args: step.args,
          disabled: step.disabled,
        })),
      });
    }, PERSIST_DELAY);
    return () => {
      if (persistTimer.current) clearTimeout(persistTimer.current);
    };
  }, [restored, input, inputEncoding, autoFire, display, steps]);

  const request = useMemo(
    () => toBakeRequest(input, inputEncoding, steps),
    [input, inputEncoding, steps],
  );

  const firedSignature = useRef<string | null>(null);

  const fire = useCallback(() => {
    if (steps.length === 0) {
      firedSignature.current = null;
      bake.reset();
      return;
    }
    // **同样的配方不跑第二遍**：还原现场、切语言、点一下别处都可能让 effect 再触发一次，
    // 每次都把输出区整块重画，在透明窗口下就是肉眼可见的闪。
    const signature = JSON.stringify(request);
    if (firedSignature.current === signature && bake.data) return;
    firedSignature.current = signature;
    bake.mutate(request);
  }, [bake, request, steps.length]);

  useEffect(() => {
    if (!restored || !autoFire) return;
    if (fireTimer.current) clearTimeout(fireTimer.current);
    fireTimer.current = setTimeout(fire, AUTO_FIRE_DELAY);
    return () => {
      if (fireTimer.current) clearTimeout(fireTimer.current);
    };
  }, [restored, autoFire, fire]);

  const result: ChefBakeResult | undefined = bake.data;
  const failedKey =
    result && result.failedAt !== null && result.failedAt !== undefined
      ? steps[visibleIndexOf(steps, result.failedAt)]?.key ?? null
      : null;

  const addOperation = useCallback((id: string, index?: number) => {
    const operation = byId.get(id);
    if (!operation) return;
    const step = stepFromOperation(operation);
    setSteps((previous) => insertAt(previous, index ?? previous.length, step));
  }, [byId]);

  const showMessage = useCallback((message: string) => {
    setNotice(message);
    setTimeout(() => setNotice(null), 4000);
  }, []);

  // 指针拖拽：按下清单条目 -> 拖到配方里松手 = 插在那个位置（见 usePickDrag 的说明）
  const dropPicked = useCallback(
    (id: string, index: number) => addOperation(id, index),
    [addOperation],
  );
  const pick = usePickDrag(steps.length, dropPicked);

  /** 清单的点击入口：拖拽之后紧跟的那次 click 要吞掉，否则拖一下会变成"拖 + 多加一步" */
  const addFromList = useCallback(
    (id: string) => {
      if (pick.consumeDrag()) return;
      addOperation(id);
    },
    [addOperation, pick],
  );

  /** 清单上的"已用次数"：只在步骤增减时变，执行结果回来不会碰它 */
  const usedCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const step of steps) counts.set(step.id, (counts.get(step.id) ?? 0) + 1);
    return counts;
  }, [steps]);

  const outputText =
    display === "hex" ? result?.outputHex ?? "" : display === "base64" ? result?.outputBase64 ?? "" : result?.output ?? "";

  /**
   * 输出区的状态行文案。
   * 刻意**不**用条件渲染：以前是"有结果画 pre、没结果画一句提示"，每次执行都把这些
   * 节点拆了重建，透明窗口里那就是看得见的闪。现在 pre 与状态行永远挂着，只改文字。
   */
  const outputStatus: { text: string; tone: "muted" | "error" } = (() => {
    if (bake.isError) return { text: bakeErrorText(bake.error), tone: "error" };
    if (result && !result.ok) {
      return {
        text: result.steps.find((step) => !step.ok)?.error ?? t("chef.ui.failed"),
        tone: "error",
      };
    }
    if (bake.isPending) return { text: t("chef.ui.running"), tone: "muted" };
    if (!result || outputText === "") return { text: t("chef.output.empty"), tone: "muted" };
    // 成功且有内容：状态行留空（节点仍在，只是没字），这样正常路径不会多出一条噪声
    return { text: "", tone: "muted" };
  })();

  const copyText = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      showMessage(t("chef.ui.clipboardBlocked", { name: label }));
    }
  };

  const importRecipe = async () => {
    try {
      const text = await navigator.clipboard.readText();
      const parsed = parseRecipe(text, byId);
      if (parsed.steps.length === 0) {
        showMessage(t("chef.recipe.importFailed"));
        return;
      }
      setSteps(parsed.steps);
      showMessage(
        parsed.rejected > 0
          ? t("chef.recipe.importedWithRejected", { n: parsed.rejected })
          : t("chef.recipe.imported"),
      );
    } catch {
      showMessage(t("chef.ui.clipboardBlocked", { name: t("chef.recipe.import") }));
    }
  };

  const saveCurrentRecipe = () => {
    const name = window.prompt?.(t("chef.recipe.savePrompt")) ?? "";
    const trimmed = name.trim();
    if (!trimmed) return;
    const next: SavedRecipe[] = [
      {
        name: trimmed,
        steps: steps.map((step) => ({ id: step.id, args: step.args, disabled: step.disabled })),
        savedAt: Date.now(),
      },
      ...saved.filter((item) => item.name !== trimmed),
    ].slice(0, 40);
    setSaved(next);
    saveSaved(next);
    showMessage(t("chef.recipe.saved", { name: trimmed }));
  };

  return (
    <div className="flex h-full min-h-0 gap-2.5">
      <Catalogue
        className="w-56 shrink-0 lg:w-64"
        operations={operations}
        labels={labels}
        usedCounts={usedCounts}
        onAdd={addFromList}
        onPickStart={pick.startPick}
        loading={operationsQuery.isLoading}
        loadError={operationsQuery.isError ? t("chef.ui.loadFailed") : null}
      />

      <Card data-testid="chef-pane-recipe" className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <CardHeader className="px-2.5 pt-2.5 pb-2">
          <IconTile icon={Workflow} toneClass="bg-primary/10 text-primary" />
          <div className="min-w-0 flex-1">
            <CardTitle className="flex items-center gap-1.5 text-xs">
              {t("chef.recipe.title")}
              <Badge variant="outline" className="tabular-nums">
                {t("chef.recipe.steps", { n: steps.filter((step) => !step.disabled).length })}
              </Badge>
            </CardTitle>
            <CardSubtitle className="mt-1">
              {/* 运行态放在这里而不是输出面板里：一眼知道这条链跑没跑过。
                  空配方时留空但保留这一行（槽位常驻），否则加第一步会让头部高度跳一下 */}
              {steps.length === 0 && !bake.isPending && !result
                ? ""
                : bake.isPending
                ? t("chef.ui.running")
                : result
                  ? result.ok
                    ? t("chef.ui.done")
                    : t("chef.ui.failed")
                  : t("chef.ui.awaitingRun")}
            </CardSubtitle>
          </div>
          <Segmented
            compact
            ariaLabel={autoFire ? t("chef.ui.autoOn") : t("chef.ui.autoOff")}
            options={[
              { value: "auto", label: t("chef.ui.autoMode"), icon: Zap },
              { value: "manual", label: t("chef.ui.manualMode"), icon: Play },
            ]}
            value={autoFire ? "auto" : "manual"}
            onChange={(next) => setAutoFire(next === "auto")}
          />
          <Button
            type="button"
            size="sm"
            variant={autoFire ? "outline" : "default"}
            className="h-7 gap-1 px-2.5 text-[11px]"
            onClick={fire}
            disabled={steps.length === 0 || bake.isPending}
          >
            <Play className="h-3 w-3" />
            {t("chef.ui.runNow")}
          </Button>
        </CardHeader>

        <Recipe
          steps={steps}
          operations={byId}
          labels={labels}
          results={result?.steps}
          failedKey={failedKey}
          pickingId={pick.pickingId}
          dropIndex={pick.dropIndex}
          setArea={pick.setArea}
          emptyTitle={t("chef.recipe.empty")}
          emptyHint={t("chef.recipe.emptyHint")}
          noArgsHint={t("chef.recipe.noArgs")}
          onChange={setSteps}
          onRemoveAt={(index) => setSteps((previous) => removeAt(previous, index))}
        />

        <CardFooter className="gap-1">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-1.5 text-[11px]"
            onClick={() => void copyText(serializeRecipe(steps), t("chef.recipe.title"))}
            disabled={steps.length === 0}
          >
            <ClipboardCopy className="mr-1 h-3 w-3" />
            {t("chef.recipe.copy")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-1.5 text-[11px]"
            onClick={() => void importRecipe()}
          >
            <ScanText className="mr-1 h-3 w-3" />
            {t("chef.recipe.import")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-1.5 text-[11px]"
            onClick={saveCurrentRecipe}
            disabled={steps.length === 0}
          >
            <Save className="mr-1 h-3 w-3" />
            {t("chef.recipe.save")}
          </Button>

          {saved.length > 0 && (
            <>
              <span className="mx-1 h-4 w-px shrink-0 bg-border" aria-hidden />
              <ul className="flex min-w-0 flex-wrap items-center gap-1" aria-label={t("chef.recipe.savedList")}>
                {saved.map((item) => (
                  <li key={item.name}>
                    <button
                      type="button"
                      onClick={() => setSteps(hydrateSteps(item.steps, byId))}
                      className="flex items-center gap-1 rounded-full border border-border/70 bg-background/60 px-2 py-0.5 text-[11px] hover:border-primary/40 hover:bg-accent"
                    >
                      {item.name}
                      <span
                        role="button"
                        tabIndex={0}
                        aria-label={t("chef.recipe.deleteSaved", { name: item.name })}
                        className="text-muted-foreground hover:text-destructive"
                        onClick={(event) => {
                          event.stopPropagation();
                          const next = saved.filter((entry) => entry.name !== item.name);
                          setSaved(next);
                          saveSaved(next);
                        }}
                        onKeyDown={(event) => {
                          if (event.key !== "Enter" && event.key !== " ") return;
                          event.preventDefault();
                          const next = saved.filter((entry) => entry.name !== item.name);
                          setSaved(next);
                          saveSaved(next);
                        }}
                      >
                        <X className="h-3 w-3" />
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}

          {/* 轻提示挤在底栏剩余空间里（常驻节点，空的时候没字）：
              原来是一条 fixed + -translate-x-1/2 + shadow-lg 的浮层，
              那三个属性任何一个都会让 WebKit 给它单独立合成层，出现/消失就是整窗闪一帧 */}
          <p
            role="status"
            aria-live="polite"
            className="min-w-0 flex-1 truncate text-right text-[11px] text-muted-foreground"
          >
            {notice ?? ""}
          </p>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 shrink-0 px-1.5 text-[11px] hover:text-destructive"
            onClick={() => setSteps([])}
            disabled={steps.length === 0}
          >
            <Trash2 className="mr-1 h-3 w-3" />
            {t("chef.recipe.clear")}
          </Button>
        </CardFooter>
      </Card>

      <div className="flex min-w-0 flex-1 flex-col gap-2.5">
        <Card data-testid="chef-pane-input" className="flex min-h-0 flex-[0_0_42%] flex-col overflow-hidden">
          <CardHeader className="px-2.5 pt-2.5 pb-2">
            <IconTile icon={TextCursorInput} toneClass="bg-sky-500/15 text-sky-600 dark:text-sky-400" />
            <div className="min-w-0 flex-1">
              <CardTitle className="text-xs">{t("chef.input.title")}</CardTitle>
              <CardSubtitle className="mt-1">{t("chef.input.hint")}</CardSubtitle>
            </div>
            <label className="flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground">
              <span className="hidden xl:inline">{t("chef.input.encoding")}</span>
              <Select
                aria-label={t("chef.input.encoding")}
                value={inputEncoding}
                onChange={(event) => setInputEncoding(event.target.value)}
                className="h-7 max-w-[8.5rem] px-1.5 text-[11px]"
              >
                {INPUT_ENCODINGS.map((encoding) => (
                  <option key={encoding} value={encoding}>
                    {labels.view(encoding)}
                  </option>
                ))}
              </Select>
            </label>
            <Badge variant="outline" className="tabular-nums">
              {t("chef.input.bytes", { n: new TextEncoder().encode(input).length })}
            </Badge>
          </CardHeader>
          <CardContent className="min-h-0 flex-1 px-2.5 pb-2.5 pt-0">
            <textarea
              value={input}
              onChange={(event) => setInput(event.target.value)}
              spellCheck={false}
              aria-label={t("chef.input.title")}
              placeholder={t("chef.input.placeholder")}
              className="pane-scroll pane-inset h-full min-h-0 w-full resize-none bg-transparent px-2.5 py-2 font-mono text-xs leading-relaxed focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            />
          </CardContent>
        </Card>

        <Card data-testid="chef-pane-output" className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <CardHeader className="px-2.5 pt-2.5 pb-2">
            <IconTile icon={Braces} toneClass="bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" />
            <div className="min-w-0 flex-1">
              <CardTitle className="text-xs">{t("chef.output.title")}</CardTitle>
              <CardSubtitle className="mt-1">{t("chef.output.hint")}</CardSubtitle>
            </div>
            <Segmented
              compact
              ariaLabel={t("chef.output.display")}
              options={OUTPUT_DISPLAYS.map((mode) => ({ value: mode, label: labels.view(mode) }))}
              value={display}
              onChange={(next) => setDisplay(next)}
            />
            <Badge variant="outline" className="tabular-nums">
              {result ? t("chef.output.bytes", { n: result.outputBytes }) : ""}
            </Badge>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 shrink-0 rounded-lg text-muted-foreground hover:text-foreground"
              onClick={() => void copyText(outputText, t("chef.output.title"))}
              disabled={!result}
              aria-label={copied ? t("chef.ui.copied") : t("chef.ui.copy")}
            >
              {copied ? <Check className="h-3.5 w-3.5" /> : <ClipboardCopy className="h-3.5 w-3.5" />}
            </Button>
          </CardHeader>
          <CardContent className="min-h-0 flex-1 overflow-hidden px-2.5 pb-2.5 pt-0">
            <div className="pane-scroll pane-inset flex h-full min-h-0 flex-col overflow-auto">
              {/* 状态行与正文都是常驻节点：只换文字，不拆了重建 */}
              <p
                data-testid="chef-output-status"
                role={outputStatus.tone === "error" ? "alert" : undefined}
                className={cn(
                  "shrink-0 border-b border-border/60 px-2.5 py-1 text-[11px] leading-snug",
                  outputStatus.tone === "error"
                    ? "bg-destructive/10 text-destructive"
                    : "text-muted-foreground",
                )}
              >
                {outputStatus.text}
              </p>
              <pre
                data-testid="chef-output"
                className={cn(
                  "min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-all px-2.5 py-2 font-mono text-xs leading-relaxed",
                  result && !result.outputUtf8 && display === "text" && "text-warning",
                )}
              >
                {outputText}
              </pre>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

/** IPC 层错误（输入超限、输入格式不认识）与算法错误不是一回事：这里只可能拿到前者 */
function bakeErrorText(error: unknown): string {
  if (error && typeof error === "object") {
    const shaped = error as { message?: unknown; code?: unknown };
    if (typeof shaped.message === "string") return shaped.message;
    if (typeof shaped.code === "string") return shaped.code;
  }
  return String(error);
}
