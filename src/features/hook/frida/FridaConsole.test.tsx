import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import type { ConsoleEvent } from "./types";
import { EventRow, EventStream } from "./FridaConsole";

const E = "\u001b";

function logEvent(line: string, msg: string): ConsoleEvent {
  return {
    id: 1,
    ts: 1_760_000_000_000,
    stream: "stdout",
    line,
    evt: { kind: "log", lvl: "log", msg },
  };
}

function renderRow(e: ConsoleEvent, raw = false) {
  return render(
    <I18nProvider>
      <EventRow e={e} raw={raw} origin={null} />
    </I18nProvider>,
  );
}

describe("Frida 控制台里的 ANSI 颜色", () => {
  const msg = `${E}[0;32m00000000${E}[0m  ${E}[0;33m00${E}[0m ${E}[0;33m00${E}[0m`;
  const wire = JSON.stringify({ t: "log", lvl: "log", msg: "\\u001b[0;32m00000000\\u001b[0m" });

  it("脚本的颜色码渲染成颜色，不再当文字显示", () => {
    const { container } = renderRow(logEvent(wire, msg));
    const text = container.textContent ?? "";
    // 这一条就是用户报的现象：[0;32m 这种片段绝不该出现在界面上
    expect(text).not.toContain("[0;32m");
    expect(text).not.toContain(E);
    expect(text).toContain("00000000");
    expect(container.querySelector(".text-emerald-400")?.textContent).toBe("00000000");
    expect(container.querySelectorAll(".text-amber-300")).toHaveLength(2);
  });

  it("原始模式仍给完整的 NDJSON 行（那是排查协议用的，不加工）", () => {
    const { container } = renderRow(logEvent(wire, msg), true);
    expect(container.textContent ?? "").toContain('"t":"log"');
  });
});

describe("事件流虚拟列表（不能靠绝对定位压行）", () => {
  /**
   * jsdom 没有布局引擎：滚动容器量出来是 0×0，虚拟列表会认为"一行都不用渲染"。
   * 所以这里给一个假视口——只为把行渲染出来，断言的是**结构**（会不会互相压行），
   * 不是像素。真实行高是否量准，仍然只能在界面上看。
   */
  function withViewport(px: number) {
    const rectOf = (el: Element): DOMRect => {
      // 滚动容器给 px 高，行给 88：88 = 一条折成 4 行的 hexdump，正是压垮绝对定位的那种行
      const h = el.hasAttribute("data-testid") && el.getAttribute("data-testid") !== "frida-stream" ? 88 : px;
      return {
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: 900,
        bottom: h,
        width: 900,
        height: h,
        toJSON: () => ({}),
      } as unknown as DOMRect;
    };
    const proto = Element.prototype;
    const oldRect = proto.getBoundingClientRect;
    const oldRO = globalThis.ResizeObserver;
    proto.getBoundingClientRect = function (this: Element) {
      return rectOf(this);
    };
    // 关键：虚拟列表靠 ResizeObserver 才拿得到视口与行高。setup 里的桩是"什么都不做"，
    // 那样一行都不会渲染 —— 这里换成"observe 即回调一次"，模拟浏览器首帧。
    class FakeRO {
      cb: ResizeObserverCallback;
      constructor(cb: ResizeObserverCallback) {
        this.cb = cb;
      }
      observe(el: Element) {
        const rect = rectOf(el);
        this.cb(
          [
            {
              target: el,
              contentRect: rect,
              borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }],
              devicePixelContentBoxSize: [],
            } as unknown as ResizeObserverEntry,
          ],
          this as unknown as ResizeObserver,
        );
      }
      unobserve() {}
      disconnect() {}
    }
    globalThis.ResizeObserver = FakeRO as unknown as typeof ResizeObserver;
    return () => {
      proto.getBoundingClientRect = oldRect;
      globalThis.ResizeObserver = oldRO;
    };
  }

  /** 用户现场：一行 hexdump 438 字符，屏宽下必然折成好几行 */
  const longMsg =
    `${E}[0;32m00000000${E}[0m  ` +
    Array.from({ length: 16 }, (_, i) => `${E}[0;33m${i.toString(16).padStart(2, "0")}${E}[0m`).join(
      " ",
    ) +
    "  ................";
  const events: ConsoleEvent[] = Array.from({ length: 400 }, (_, i) => ({
    id: i + 1,
    ts: 1_760_000_000_000 + i * 10,
    stream: "stdout" as const,
    line: JSON.stringify({ t: "log", lvl: "log", msg: longMsg }),
    evt: { kind: "log", lvl: "log", msg: longMsg },
  }));

  it("只渲染窗口内的行、且每行都挂着测量契约", () => {
    const restore = withViewport(600);
    try {
      render(
        <I18nProvider>
          <EventStream
            events={events}
            rawMode={false}
            origin={null}
            running={true}
            emptyText="空"
          />
        </I18nProvider>,
      );
      const rows = screen.getAllByTestId("frida-row");
      expect(rows.length).toBeGreaterThan(0);
      expect(rows.length).toBeLessThan(events.length); // 确实虚拟化了
      for (const row of rows) expect(row).toHaveAttribute("data-index");
    } finally {
      restore();
    }
  });

  it("可见行走普通文档流：量错行高顶多滚动条不准，绝不允许把下一行压在身上", () => {
    const restore = withViewport(600);
    try {
      const { container } = render(
        <I18nProvider>
          <EventStream
            events={events}
            rawMode={false}
            origin={null}
            running={true}
            emptyText="空"
          />
        </I18nProvider>,
      );
      const rows = screen.getAllByTestId("frida-row");
      for (const row of rows) {
        const style = (row as HTMLElement).style;
        // 曾经的形状：position:absolute + translateY(start) —— 一旦高度靠估算就必然压行
        expect(style.position).not.toBe("absolute");
        expect(String(style.transform)).not.toContain("translateY");
      }
      // 上下留白把"没渲染的行"垫出来：滚动条长度不能塌成一屏
      const box = container.querySelector("[data-testid=\"frida-rows\"]") as HTMLElement;
      // 列表停在顶部时 paddingTop 为 0 是对的；关键是"没渲染的行"仍然占着高度，
      // 否则 400 条日志的滚动条会塌成一屏
      expect(parseFloat(box.style.paddingBottom)).toBeGreaterThan(0);
      expect(parseFloat(box.style.paddingTop)).toBeGreaterThanOrEqual(0);
    } finally {
      restore();
    }
  });

  it("长行按整词折行，不在 hexdump 的字节中间断开", () => {
    const { container } = renderRow(logEvent("{}", longMsg));
    const line = container.querySelector(".group") as HTMLElement;
    expect(line.className).toContain("[overflow-wrap:anywhere]");
    expect(line.className).not.toContain("break-all");
    expect(line.className).toContain("whitespace-pre-wrap");
  });
});
