import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useDragDropPath } from "./useDragDropPath";

/**
 * jsdom 无 __TAURI_INTERNALS__：hook 必须安静跳过（不订阅、不抛错、
 * 不回调），页面保持手填输入框可用。Tauri 真实环境的拖放路径由
 * webview onDragDropEvent 提供（dev 浏览器不覆盖）。
 */
describe("useDragDropPath", () => {
  it("非 Tauri 环境：不抛错、不回调", async () => {
    const onPath = vi.fn();
    const { unmount } = renderHook(() =>
      useDragDropPath({ onPath, extensions: ["so"], enabled: true }),
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(onPath).not.toHaveBeenCalled();
    expect(() => unmount()).not.toThrow();
  });

  it("enabled=false：完全不订阅", async () => {
    const onPath = vi.fn();
    renderHook(() => useDragDropPath({ onPath, enabled: false }));
    await new Promise((r) => setTimeout(r, 20));
    expect(onPath).not.toHaveBeenCalled();
  });
});
