/**
 * 配方的纯逻辑：步骤装配、拖拽插入位置、持久化。
 * 单独成文件是为了能在 node 环境里直接测——界面测试只该管"看得见的东西"。
 */

import type {
  ChefBakeRequest,
  ChefOperationDto,
  ChefStepInput,
} from "@/api/chef";

/** 界面里的一步：`key` 只为 React 的列表稳定性服务，不参与 IPC */
export interface RecipeStep {
  key: string;
  id: string;
  args: Record<string, string>;
  /** 关掉但不删除：中途想对比"加没加这一步"时不用来回拖 */
  disabled: boolean;
}

const WORKBENCH_KEY = "chef.workbench.v1";
const SAVED_KEY = "chef.saved.v1";

let sequence = 0;
function nextKey(): string {
  sequence += 1;
  return `s${Date.now().toString(36)}-${sequence}`;
}

/** 新步骤的参数从注册表默认值起步：界面显示什么，后端就收到什么 */
export function stepFromOperation(operation: ChefOperationDto): RecipeStep {
  const args: Record<string, string> = {};
  for (const arg of operation.args) {
    args[arg.name] = arg.default;
  }
  return { key: nextKey(), id: operation.id, args, disabled: false };
}

export function insertAt<T>(list: T[], index: number, item: T): T[] {
  const next = list.slice();
  next.splice(Math.max(0, Math.min(index, next.length)), 0, item);
  return next;
}

export function moveBy(list: RecipeStep[], index: number, delta: number): RecipeStep[] {
  const target = index + delta;
  if (target < 0 || target >= list.length) return list;
  const next = list.slice();
  const [step] = next.splice(index, 1);
  next.splice(target, 0, step);
  return next;
}

export function removeAt(list: RecipeStep[], index: number): RecipeStep[] {
  return list.filter((_, i) => i !== index);
}

/** 交后端时剔掉被关掉的步骤（顺序保持原样） */
export function toBakeRequest(
  input: string,
  inputEncoding: string,
  steps: RecipeStep[],
): ChefBakeRequest {
  const recipe: ChefStepInput[] = steps
    .filter((step) => !step.disabled)
    .map((step) => ({ id: step.id, args: { ...step.args } }));
  return { input, inputEncoding, recipe };
}

/** 界面里的第 n 个可见步骤对应 bake 结果里的第几步（关掉步骤后编号会错位） */
export function visibleIndexOf(steps: RecipeStep[], visible: number): number {
  let seen = -1;
  for (let i = 0; i < steps.length; i += 1) {
    if (!steps[i].disabled) {
      seen += 1;
      if (seen === visible) return i;
    }
  }
  return -1;
}


/** 折叠行上的参数摘要项：value 是原值，shown 是 chip 上放得下的写法 */
export interface StepArgSummary {
  name: string;
  kind: string;
  value: string;
  shown: string;
}


/**
 * chip 上显示的值：choice 走词典出译名（cbc -> 工作模式里的"链式 CBC"这类），
 * 被截断的长值原样显示（词典里不会有半个值）。
 * 传进来的 option() 是 labels 的取词函数，recipe.ts 因此不依赖 React。
 */
export function chipValue(
  entry: StepArgSummary,
  option: (token: string) => string,
): string {
  if (entry.kind !== "choice" || entry.value !== entry.shown) return entry.shown;
  return option(entry.value);
}

/**
 * 与注册表默认值**不同**的参数。
 * 为什么只列这些：一条 AES 链里 8 个参数有 6 个没动过，全列出来等于没列；
 * 用户要确认的是"我改了什么"。值太长截断，完整值展开后在输入框里看。
 */
export function nonDefaultArgs(
  step: RecipeStep,
  operation: ChefOperationDto,
): StepArgSummary[] {
  const changed: StepArgSummary[] = [];
  for (const arg of operation.args) {
    const value = step.args[arg.name] ?? arg.default;
    if (value === arg.default) continue;
    const shown = value.length > 10 ? value.slice(0, 9) + "..." : value;
    changed.push({ name: arg.name, kind: arg.kind, value, shown });
  }
  return changed;
}

export function serializeRecipe(steps: RecipeStep[]): string {
  return JSON.stringify(
    steps.map((step) => ({ id: step.id, args: step.args, disabled: step.disabled })),
    null,
    2,
  );
}

/**
 * 粘回来的配方要能容错：只认识 `{id, args}` 形状，其余静默丢掉并回报数量。
 * 报错太狠会让"从别人那抄一条链"这件事变成一次调试。
 */
export function parseRecipe(
  raw: string,
  operations: Map<string, ChefOperationDto>,
): { steps: RecipeStep[]; rejected: number } {
  const parsed: unknown = (() => {
    try {
      return JSON.parse(raw);
    } catch {
      return null;
    }
  })();
  const list = Array.isArray(parsed)
    ? parsed
    : parsed && typeof parsed === "object" && Array.isArray((parsed as { recipe?: unknown }).recipe)
      ? ((parsed as { recipe: unknown[] }).recipe as unknown[])
      : null;
  if (!list) return { steps: [], rejected: 0 };
  const steps: RecipeStep[] = [];
  let rejected = 0;
  for (const entry of list) {
    const id =
      typeof entry === "string"
        ? entry
        : entry && typeof entry === "object"
          ? String((entry as { id?: unknown }).id ?? "")
          : "";
    const operation = operations.get(id);
    if (!operation) {
      rejected += 1;
      continue;
    }
    const step = stepFromOperation(operation);
    if (entry && typeof entry === "object") {
      const given = (entry as { args?: Record<string, unknown> }).args;
      if (given && typeof given === "object") {
        for (const arg of operation.args) {
          const value = given[arg.name];
          if (typeof value === "string") step.args[arg.name] = value;
          else if (typeof value === "boolean" || typeof value === "number") {
            step.args[arg.name] = String(value);
          }
        }
      }
      step.disabled = Boolean((entry as { disabled?: unknown }).disabled);
    }
    steps.push(step);
  }
  return { steps, rejected };
}

// ---------------------------------------------------------------------------
// 持久化：当前工作台状态 + 已保存配方（CyberChef 的 Favourites 的本地版）
// ---------------------------------------------------------------------------

export interface WorkbenchState {
  input: string;
  inputEncoding: string;
  autoFire: boolean;
  display: string;
  steps: { id: string; args: Record<string, string>; disabled: boolean }[];
}

export interface SavedRecipe {
  name: string;
  steps: WorkbenchState["steps"];
  savedAt: number;
}

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    // WKWebView 隐私模式下访问 localStorage 会抛：状态退回内存，别让整个页面挂掉
    return null;
  }
}

/** 读回来的步骤补上 key / 用注册表默认值补齐缺的参数 */
export function hydrateSteps(
  saved: WorkbenchState["steps"],
  operations: Map<string, ChefOperationDto>,
): RecipeStep[] {
  const out: RecipeStep[] = [];
  for (const item of saved) {
    const operation = operations.get(item.id);
    if (!operation) continue;
    const step = stepFromOperation(operation);
    for (const arg of operation.args) {
      const given = item.args?.[arg.name];
      if (typeof given === "string") step.args[arg.name] = given;
    }
    step.disabled = Boolean(item.disabled);
    out.push(step);
  }
  return out;
}

export function loadWorkbench(): WorkbenchState | null {
  const store = storage();
  if (!store) return null;
  try {
    const raw = store.getItem(WORKBENCH_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<WorkbenchState>;
    return {
      input: typeof parsed.input === "string" ? parsed.input : "",
      inputEncoding: typeof parsed.inputEncoding === "string" ? parsed.inputEncoding : "utf8",
      autoFire: parsed.autoFire !== false,
      display: typeof parsed.display === "string" ? parsed.display : "text",
      steps: Array.isArray(parsed.steps) ? parsed.steps : [],
    };
  } catch {
    return null;
  }
}

export function saveWorkbench(state: WorkbenchState): void {
  const store = storage();
  if (!store) return;
  try {
    store.setItem(WORKBENCH_KEY, JSON.stringify(state));
  } catch {
    // 存不下（配额/隐私模式）只影响下次打开时不回填，不该打断当前工作
  }
}

export function loadSaved(): SavedRecipe[] {
  const store = storage();
  if (!store) return [];
  try {
    const raw = store.getItem(SAVED_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? (parsed as SavedRecipe[]) : [];
  } catch {
    return [];
  }
}

export function saveSaved(list: SavedRecipe[]): void {
  const store = storage();
  if (!store) return;
  try {
    store.setItem(SAVED_KEY, JSON.stringify(list));
  } catch {
    /* 同上 */
  }
}

export function operationsById(list: ChefOperationDto[]): Map<string, ChefOperationDto> {
  return new Map(list.map((operation) => [operation.id, operation]));
}
