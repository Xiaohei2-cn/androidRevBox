import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";

/**
 * `/proc` 这一栏的行为契约（设备信息页）：
 * ① 渲染时**一条读取都不发**（以前每次刷新都白跑一次注定失败的 maps 读取）；
 * ② maps 标成"未读取"而不是红色的"不可读"，并给一个箭头；
 * ③ 点了才发请求，且带对 pid/file；
 * ④ 详情里必须把"以 root 读取"和"共 N 行"显示出来——用户想知道的就是这两件事。
 */

const procRead = vi.hoisted(() => vi.fn());

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return { ...actual, deviceApi: { ...actual.deviceApi, procRead } };
});

import { ProcPaths } from "./ProcPaths";

const ENTRIES = [
  { name: "maps", path: "/proc/30417/maps", summary: null, readable: false, readOnDemand: true },
  {
    name: "cmdline",
    path: "/proc/30417/cmdline",
    summary: "com.google.android.apps.nexuslauncher",
    readable: true,
    readOnDemand: false,
  },
];

function renderSelf() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nProvider>
        <ProcPaths serial="SERIAL-1" pid="30417" entries={ENTRIES} />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  procRead.mockReset();
  procRead.mockResolvedValue({
    path: "/proc/30417/maps",
    file: "maps",
    total_lines: 2566,
    returned_lines: 2,
    truncated: true,
    text: "12c00000-22c00000 rw-p\n32c00000-42c00000 rw-p",
    read_via: "root",
  });
});

describe("ProcPaths（按需读取）", () => {
  it("渲染时不发起任何 /proc 读取", () => {
    renderSelf();
    expect(procRead).not.toHaveBeenCalled();
  });

  it("maps 显示成「未读取」并带箭头，而不是红色的不可读", () => {
    renderSelf();
    const row = screen.getByTestId("fg-proc-open-SERIAL-1-maps");
    expect(row).toBeTruthy();
    // 这一项根本没读，不能出现"不可读"那句
    const list = screen.getByTestId("fg-proc-paths-SERIAL-1");
    expect(list.textContent ?? "").not.toContain("不可读");
    expect(list.textContent ?? "").toContain("未读取");
    // cmdline 是读到的，摘要照常显示
    expect(list.textContent ?? "").toContain("nexuslauncher");
  });

  it("点箭头才读，并且带对 pid/file", async () => {
    renderSelf();
    fireEvent.click(screen.getByTestId("fg-proc-open-SERIAL-1-maps"));
    await waitFor(() => expect(procRead).toHaveBeenCalledTimes(1));
    expect(procRead).toHaveBeenCalledWith("SERIAL-1", 30417, "maps", 400);
    const meta = await screen.findByTestId("fg-proc-detail-meta");
    expect(meta.textContent ?? "").toContain("2566");
    expect(meta.textContent ?? "").toContain("以 root 读取");
    const body = await screen.findByTestId("fg-proc-detail-body");
    expect(body.textContent ?? "").toContain("12c00000-22c00000");
  });

  it("返回回到列表；没有 pid 时箭头不可点", async () => {
    renderSelf();
    fireEvent.click(screen.getByTestId("fg-proc-open-SERIAL-1-maps"));
    await screen.findByTestId("fg-proc-detail-body");
    fireEvent.click(screen.getByTestId("fg-proc-detail-back"));
    await waitFor(() =>
      expect(screen.queryByTestId("fg-proc-detail-body")).toBeNull(),
    );
    expect(screen.getByTestId("fg-proc-paths-SERIAL-1")).toBeTruthy();

    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <I18nProvider>
          <ProcPaths serial="SERIAL-2" pid={null} entries={ENTRIES} />
        </I18nProvider>
      </QueryClientProvider>,
    );
    const disabled = screen.getByTestId("fg-proc-open-SERIAL-2-maps") as HTMLButtonElement;
    expect(disabled.disabled).toBe(true);
    expect(procRead).toHaveBeenCalledTimes(1);
  });
});
