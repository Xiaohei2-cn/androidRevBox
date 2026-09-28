import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import { ExportControl } from "./FridaConsole";
import type { ConsoleEvent } from "./types";

vi.mock("@/api/dialog", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  hasNativeDialog: vi.fn(() => true),
  saveFile: vi.fn(),
}));
vi.mock("@/api/hook", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  hookApi: { exportSave: vi.fn() },
}));

import { hookApi } from "@/api/hook";
import { hasNativeDialog, saveFile } from "@/api/dialog";

function evt(id: number, line: string): ConsoleEvent {
  return { id, ts: 1_760_000_000_000 + id, stream: "stdout", line, evt: { kind: "log", lvl: "log", msg: line } };
}

function renderControl(events: ConsoleEvent[]) {
  return render(
    <I18nProvider>
      <ExportControl events={events} sessionName="attach com.x · hook.js" />
    </I18nProvider>,
  );
}

describe("控制台「保存日志」", () => {
  beforeEach(() => {
    vi.mocked(saveFile).mockReset();
    vi.mocked(hookApi.exportSave).mockReset();
    vi.mocked(hasNativeDialog).mockReturnValue(true);
  });

  /**
   * 这条钉的是"存的就是屏幕上这些行"：调用方传进来的就是过滤后的 `visible`，
   * 组件不许自己再去拿全量（那会让"先筛再留底"变成一句空话）。
   */
  it("保存的正文只来自传进来的那一批行（当前过滤结果）", async () => {
    vi.mocked(saveFile).mockResolvedValue("/tmp/frida.log");
    vi.mocked(hookApi.exportSave).mockResolvedValue(9);
    renderControl([evt(1, "keep me"), evt(2, "also keep")]);
    fireEvent.click(screen.getByTestId("frida-export"));
    await waitFor(() => expect(hookApi.exportSave).toHaveBeenCalledTimes(1));
    const [path, content] = vi.mocked(hookApi.exportSave).mock.calls[0];
    expect(path).toBe("/tmp/frida.log");
    expect(content).toContain("keep me");
    expect(content).toContain("also keep");
    expect(content).not.toContain("dropped by the filter");
    expect(screen.getByTestId("frida-export-outcome").textContent).toContain("已保存 2 行");
  });

  /** 取消不是失败：以前踩过——把"我没选"报成"保存失败"，用户会以为工具坏了。 */
  it("用户取消保存框：什么都不写，也不报失败", async () => {
    vi.mocked(saveFile).mockResolvedValue(null);
    renderControl([evt(1, "line")]);
    fireEvent.click(screen.getByTestId("frida-export"));
    await waitFor(() => expect(saveFile).toHaveBeenCalled());
    expect(hookApi.exportSave).not.toHaveBeenCalled();
    expect(screen.queryByTestId("frida-export-outcome")).toBeNull();
  });

  it("浏览器预览里没有原生对话框，要照实说，不许冒充成取消", async () => {
    vi.mocked(hasNativeDialog).mockReturnValue(false);
    vi.mocked(saveFile).mockResolvedValue(null);
    renderControl([evt(1, "line")]);
    fireEvent.click(screen.getByTestId("frida-export"));
    await waitFor(() =>
      expect(screen.getByTestId("frida-export-outcome").textContent).toContain("桌面版"),
    );
    expect(hookApi.exportSave).not.toHaveBeenCalled();
  });

  it("没有可保存的行时按钮不可点（别生成一个空文件让人以为成功）", () => {
    renderControl([]);
    expect(screen.getByTestId("frida-export")).toBeDisabled();
  });

  it("落盘失败要把原因说清（路径把关在桌面端，那句人话原样传上来）", async () => {
    vi.mocked(saveFile).mockResolvedValue("/tmp/frida.sh");
    vi.mocked(hookApi.exportSave).mockRejectedValue(new Error("只支持保存为 log / txt / json"));
    renderControl([evt(1, "line")]);
    fireEvent.click(screen.getByTestId("frida-export"));
    await waitFor(() =>
      expect(screen.getByTestId("frida-export-outcome").textContent).toContain("只支持保存为 log"),
    );
  });
});
