import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// vitest globals:false 下 RTL 无法自动注册清理，手动在每个用例后卸载 DOM
afterEach(() => {
  cleanup();
});

// Node 26 运行时不提供 localStorage（需 --localstorage-file），jsdom 环境下补一个内存实现
if (typeof globalThis.localStorage === "undefined") {
  const store = new Map<string, string>();
  globalThis.localStorage = {
    getItem: (key) => (store.has(key) ? store.get(key)! : null),
    setItem: (key, value) => {
      store.set(key, String(value));
    },
    removeItem: (key) => {
      store.delete(key);
    },
    clear: () => store.clear(),
    key: (index) => Array.from(store.keys())[index] ?? null,
    get length() {
      return store.size;
    },
  } as Storage;
}

// jsdom 未实现 matchMedia，主题跟随系统逻辑依赖它
if (typeof window.matchMedia !== "function") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}


/*
 * R-01 的代价：jsdom 没有 canvas，而 xterm 6 的核心在解析 CSS 颜色时会借 2D 上下文做一次
 * fillStyle 往返归一化（`@xterm/xterm/src/common/Color.ts`）。没有这个桩，连"缓冲区里
 * 有没有这个颜色"都测不了。
 *
 * 这个桩只做一件事：**存进去取得出来**。它不代表渲染正确 —— 像素那半归
 * docs/frida_terminal_phases.md §5 的手工矩阵，别把这份测试当成"渲染测过了"。
 */
if (typeof window !== "undefined" && typeof window.HTMLCanvasElement !== "undefined") {
  Object.defineProperty(window.HTMLCanvasElement.prototype, "getContext", {
    configurable: true,
    writable: true,
    value(this: HTMLCanvasElement) {
      const ctx: Record<string, unknown> = {
        canvas: this,
        fillStyle: "#000000",
        strokeStyle: "#000000",
        globalAlpha: 1,
        globalCompositeOperation: "source-over",
        font: "10px monospace",
        textBaseline: "alphabetic",
        measureText: (text: string) => ({
          width: String(text).length * 6,
          actualBoundingBoxAscent: 8,
          actualBoundingBoxDescent: 2,
        }),
      };
      for (const noop of [
        "fillRect", "clearRect", "strokeRect", "fillText", "strokeText",
        "save", "restore", "beginPath", "closePath", "moveTo", "lineTo",
        "fill", "stroke", "clip", "arc", "drawImage", "putImageData", "translate", "scale",
      ]) {
        ctx[noop] = () => undefined;
      }
      ctx["createLinearGradient"] = () => ({ addColorStop: () => undefined });
      ctx["getImageData"] = (_x: number, _y: number, w: number, h: number) => ({
        data: new Uint8ClampedArray(Math.max(1, w * h * 4)),
        width: w,
        height: h,
      });
      return ctx;
    },
  });
}

/*
 * jsdom 没有 PointerEvent 构造器。fireEvent.pointerDown/Move/Up 于是退化成**不带坐标**的
 * Event，任何依赖 clientX/clientY 的交互（算法清单的指针拖拽）在测试里读到的永远是
 * undefined —— 表现是"测试全绿但拖不动"或反过来。用 MouseEvent 顶上：坐标、button、
 * bubbles 都对齐真实行为。
 */
if (typeof window !== "undefined" && typeof window.PointerEvent === "undefined") {
  window.PointerEvent = window.MouseEvent as unknown as typeof window.PointerEvent;
}

// radix 组件在 jsdom 中依赖 ResizeObserver
if (typeof globalThis.ResizeObserver !== "function") {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

// jsdom 未实现 scrollIntoView（P7 设置页跳转定位使用），stub 掉
if (typeof Element.prototype.scrollIntoView !== "function") {
  Element.prototype.scrollIntoView = () => {};
}

// i18n：测试统一固定 zh-CN（jsdom 默认 navigator.language=en-US 会让词典探测漂移，
// 断言中文文案的用例必须确定性）
Object.defineProperty(window.navigator, "language", {
  configurable: true,
  get: () => "zh-CN",
});
