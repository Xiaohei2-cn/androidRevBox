/**
 * 透明区点击穿透的判定层（第五十四轮）。
 *
 * 一句话规则：**鼠标底下那一点有没有"实体"**。有实体（卡片、按钮、输入框、可选中的文字、
 * 窗口 chrome）就接点击；只有半透明底色（留白、卡片之间的间隙、圆角外）就把点击让给后面的 App。
 *
 * 为什么判定必须在这儿而不是 CSS 里：整窗穿透是 macOS 的窗口级开关，一旦开了，
 * webview 就收不到鼠标事件了 —— 所以不能靠 :hover/pointer-events 反应，只能拿
 * Rust 读到的全局光标位置，反过来问 DOM「这一点是什么」。`elementFromPoint` 不需要
 * 窗口拿到事件，问得动。
 */

/** 全局底色/布局层：它们"看着透明"正是因为不代表任何可点内容，所以不算实体 */
export const TINT_LAYERS = ".app-surface,.page-canvas";

/**
 * 算"实体"的原生可交互元素。
 * React 的 onClick 在 DOM 上查不到（事件挂在根节点），所以能靠的只有语义标签、
 * ARIA role，以及我们自己标的 data-* —— 这也是标题栏/tab 栏要显式标记的原因。
 */
export const INTERACTIVE_SELECTOR = [
  "button",
  "a[href]",
  "input",
  "select",
  "textarea",
  "summary",
  "label",
  "[role]",
  "[contenteditable='true']",
  "[data-clickable]",
].join(",");

/**
 * 背景 alpha 低于它就当作"这块地方本来也是透的"。
 * 0.15 这条线画在：卡片/输入框（实色）算实体，而 3% 那一层极浅着色不算。
 */
export const PAINT_ALPHA = 0.15;

/** 判定一个元素栈需要的三种知识；注入进来是为了能在没有布局引擎的环境里逐条钉住 */
export interface StackJudges {
  isTint: (el: Element) => boolean;
  isInteractive: (el: Element) => boolean;
  isPainted: (el: Element) => boolean;
}

/**
 * 从上往下找第一个"说得清这一点是实体"的元素（纯函数）。
 *
 * 栈里越靠前越是压在上面的元素；一路都是底色层/无边框的布局容器，才算透明。
 * 显式标记优先于任何启发式：`data-click-through="solid"` 钉住（窗口 chrome 用），
 * `="pass"` 钉透（以后要挖洞也给这条路，不用再改判定逻辑）。
 */
export function isSolidStack(elements: readonly Element[], judges: StackJudges): boolean {
  for (const el of elements) {
    const forced = el.getAttribute("data-click-through");
    if (forced === "solid") return true;
    if (forced === "pass") return false;
    if (judges.isTint(el)) continue;
    if (judges.isInteractive(el)) return true;
    if (judges.isPainted(el)) return true;
  }
  return false;
}

/** `rgb(24 24 27 / 0.6)`、`rgba(0,0,0,.6)`、`hsl(0 0% 0% / 60%)`、`transparent` → alpha */
export function alphaOf(color: string): number {
  const value = (color ?? "").trim().toLowerCase();
  if (!value || value === "none" || value === "transparent") return 0;
  // 先摘掉收尾的 ")"：`hsl(0 0% 0% / 35%)` 的 alpha 段是 "35%"，带着括号就认不出百分号了
  const body = value.replace(/\)\s*$/, "");
  const slash = body.lastIndexOf("/");
  if (slash > 0) {
    const token = body.slice(slash + 1).trim();
    const percent = token.endsWith("%");
    const raw = Number.parseFloat(percent ? token.slice(0, -1) : token);
    if (!Number.isFinite(raw)) return 1;
    return percent ? raw / 100 : raw;
  }
  // 老式逗号写法：第四个分量才是 alpha；只有三个分量就是全不透明
  const nums = value.match(/-?\d*\.?\d+/g);
  if (!nums || nums.length < 4) return 1;
  const raw = Number.parseFloat(nums[3]);
  return Number.isFinite(raw) ? raw : 1;
}

/** 浏览器实现：`matches` + computed style */
export function domJudges(): StackJudges {
  return {
    isTint: (el) => el.matches(TINT_LAYERS),
    isInteractive: (el) => el.matches(INTERACTIVE_SELECTOR),
    isPainted: (el) => {
      const style = getComputedStyle(el as HTMLElement);
      if (alphaOf(style.backgroundColor) >= PAINT_ALPHA) return true;
      if (style.backgroundImage && style.backgroundImage !== "none") return true;
      // 图片/canvas 本身就是实体，哪怕没有背景色
      return el instanceof HTMLImageElement || el instanceof HTMLCanvasElement;
    },
  };
}

/**
 * 这一点该不该接点击。
 *
 * 拿不到元素栈（无布局引擎、坐标在视口外、jsdom 未实现 `elementFromPoint`）时
 * **一律算实体**：宁可"透传没生效"，也不能把用户自己的界面点不到。
 */
export function isSolidAt(x: number, y: number, doc: Document = document): boolean {
  const stack = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(x, y) : [];
  if (!stack || stack.length === 0) return true;
  return isSolidStack(stack, domJudges());
}
