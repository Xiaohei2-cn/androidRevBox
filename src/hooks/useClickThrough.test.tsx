import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ pointerState: vi.fn(), setClickThrough: vi.fn() }));
vi.mock("@/api/window", () => ({ windowApi: api }));

import { useClickThrough } from "./useClickThrough";

/**
 * 造一个"被显式声明为可穿透"的元素栈（今天的现实形状 = 左侧 tab 栏的透明留白）：
 * jsdom 没有布局引擎，`elementsFromPoint` 得自己接。
 * 判定改成"默认接住、只有 data-click-through=pass 才让出"之后，这里必须标 pass，
 * 否则测的就不是穿透路径了。
 */
function passableStack() {
  const rail = document.createElement("nav");
  rail.setAttribute("data-click-through", "pass");
  const surface = document.createElement("div");
  surface.className = "app-surface";
  return [rail, surface, document.documentElement, document.body];
}

function solidStack() {
  const button = document.createElement("button");
  return [button, document.documentElement, document.body];
}

const originalFromPoint = document.elementsFromPoint;

describe("useClickThrough（透明区点击穿透的状态机）", () => {
  beforeEach(() => {
    api.pointerState.mockReset();
    api.setClickThrough.mockReset();
    api.setClickThrough.mockResolvedValue(true);
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    document.elementsFromPoint = originalFromPoint;
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("开关关着时不打扰窗口层", () => {
    renderHook(() => useClickThrough(false));
    expect(api.pointerState).not.toHaveBeenCalled();
  });

  it("鼠标在声明可穿透的透明处 → 开穿透；回到实体上 → 收回", async () => {
    document.elementsFromPoint = () => passableStack() as unknown as Element[];
    api.pointerState.mockResolvedValue({ x: 100, y: 100, inside: true });
    renderHook(() => useClickThrough(true));
    await act(async () => {
      vi.advanceTimersByTime(0);
    });
    expect(api.setClickThrough).toHaveBeenLastCalledWith(true);

    // 移到按钮上：必须马上变回可点击，否则自己的界面就点不到了
    document.elementsFromPoint = () => solidStack() as unknown as Element[];
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(api.setClickThrough).toHaveBeenLastCalledWith(false);
  });

  it("内容区没显式声明就不穿透：界面点不出去（用户报的第一条）", async () => {
    const card = document.createElement("div");
    card.className = "bg-card"; // 实色卡片：不标 pass 就必须接住
    const host = document.createElement("div");
    host.className = "absolute inset-0 p-5";
    document.elementsFromPoint = () =>
      [card, host, document.querySelector(".app-surface") ?? document.createElement("div"), document.documentElement] as unknown as Element[];
    api.pointerState.mockResolvedValue({ x: 100, y: 100, inside: true });
    renderHook(() => useClickThrough(true));
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(api.setClickThrough).not.toHaveBeenCalledWith(true);
  });

  it("光标不在窗口内时不改状态（不然状态会来回抖）", async () => {
    document.elementsFromPoint = () => passableStack() as unknown as Element[];
    api.pointerState.mockResolvedValue({ x: -5, y: 4000, inside: false });
    renderHook(() => useClickThrough(true));
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(api.setClickThrough).not.toHaveBeenCalled();
  });

  it("问不到指针就按可点击处理，并立刻收回穿透", async () => {
    document.elementsFromPoint = () => passableStack() as unknown as Element[];
    api.pointerState.mockResolvedValue({ x: 100, y: 100, inside: true });
    renderHook(() => useClickThrough(true));
    await act(async () => {
      vi.advanceTimersByTime(0);
    });
    expect(api.setClickThrough).toHaveBeenLastCalledWith(true);

    api.pointerState.mockRejectedValue(new Error("IPC 断了"));
    api.setClickThrough.mockClear();
    await act(async () => {
      vi.advanceTimersByTime(160);
    });
    expect(api.setClickThrough).toHaveBeenCalledWith(false);
  });

  it("卸载（关掉开关/切页）必须收回穿透状态", async () => {
    document.elementsFromPoint = () => passableStack() as unknown as Element[];
    api.pointerState.mockResolvedValue({ x: 100, y: 100, inside: true });
    const { unmount } = renderHook(() => useClickThrough(true));
    await act(async () => {
      vi.advanceTimersByTime(0);
    });
    api.setClickThrough.mockClear();
    unmount();
    await act(async () => {
      vi.advanceTimersByTime(0);
    });
    expect(api.setClickThrough).toHaveBeenCalledWith(false);
  });
});
