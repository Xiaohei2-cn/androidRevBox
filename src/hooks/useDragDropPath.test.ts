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

/**
 * Tauri 环境下的真实分发：把 webview 的拖放事件桩出来，直接喂一次 drop。
 * 这一段是有必要的 —— 上传/替换两条链路都靠它拿路径，只测"非 Tauri 不炸"等于没测。
 */
describe("useDragDropPath（Tauri 环境）", () => {
  it("多选拖入 → onPaths 收到全部；只要扩展名匹配的", async () => {
    const paths = ["/Users/x/Downloads/a.so", "/Users/x/Downloads/b.so", "/Users/x/Notes.txt"];
    let handler: ((e: unknown) => void) | undefined;
    const onPaths = vi.fn();
    await (async () => {
      vi.doMock("@tauri-apps/api/webview", () => ({
        getCurrentWebview: () => ({
          onDragDropEvent: async (cb: (e: unknown) => void) => {
            handler = cb;
            return () => {
              handler = undefined;
            };
          },
        }),
      }));
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
      const { useDragDropPath: hook } = await import("./useDragDropPath");
      const { unmount } = renderHook(() => hook({ onPaths, extensions: ["so"], enabled: true }));
      await vi.waitFor(() => expect(handler).toBeTypeOf("function"));
      handler?.({ payload: { type: "drop", paths } });
      expect(onPaths).toHaveBeenCalledWith([paths[0], paths[1]]);
      // drop 之外的事件（enter/over/leave）不该回调
      handler?.({ payload: { type: "over", paths } });
      expect(onPaths).toHaveBeenCalledTimes(1);
      unmount();
      expect(handler).toBeUndefined();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      vi.doUnmock("@tauri-apps/api/webview");
    })();
  });

  it("只给 onPath 时保持旧行为：回调第一个匹配项", async () => {
    let handler: ((e: unknown) => void) | undefined;
    const onPath = vi.fn();
    vi.doMock("@tauri-apps/api/webview", () => ({
      getCurrentWebview: () => ({
        onDragDropEvent: async (cb: (e: unknown) => void) => {
          handler = cb;
          return () => {};
        },
      }),
    }));
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { useDragDropPath: hook } = await import("./useDragDropPath");
    renderHook(() => hook({ onPath }));
    await vi.waitFor(() => expect(handler).toBeTypeOf("function"));
    handler?.({ payload: { type: "drop", paths: ["/a/one.bin", "/a/two.bin"] } });
    expect(onPath).toHaveBeenCalledWith("/a/one.bin");
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    vi.doUnmock("@tauri-apps/api/webview");
  });
});
