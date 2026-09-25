import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { PreflightDto } from "@/api/hook";
import { PreflightList } from "./SettingsPanel";

const T = (k: string) => (k === "hook.frida.channel" ? "frida 通道（握手）" : k);

function base(over: Partial<PreflightDto>): PreflightDto {
  return {
    adbOk: true,
    adbHint: null,
    pythonOk: true,
    pythonHint: null,
    pythonPath: "/venv/bin/python3",
    fridaOk: true,
    fridaVersion: "16.5.7",
    fridaHint: null,
    runnerOk: true,
    runnerHint: null,
    ...over,
  } as PreflightDto;
}

function list(preflight: PreflightDto) {
  return render(<PreflightList preflight={preflight} remoteMode={false} t={T} onFix={vi.fn()} />);
}

describe("preflight 的「frida 通道」项（三态）", () => {
  it("没探（Python 未就绪）时这一项不出现 —— 红字是噪音，绿勾是撒谎", () => {
    const preflight = base({ pythonOk: false, pythonHint: "未配置", channelOk: null, channelHint: null });
    list(preflight);
    expect(screen.queryByText("frida 通道（握手）")).toBeNull();
  });

  it("探过且失败：整行标红，并把原因挂在 title 上（版本不匹配要一眼看得见）", () => {
    const hint =
      "连不上设备上的 frida-server：ProtocolError unable to communicate…（工具用的 frida 是 16.5.7，两端主版本必须一致）";
    const { container } = list(base({ channelOk: false, channelHint: hint }));
    const row = screen.getByText("frida 通道（握手）");
    expect(row).toBeInTheDocument();
    expect(row.getAttribute("title")).toBe(hint);
    // 这一项自己必须是红的（别的项照样可以绿：整体图标不是这里要钉的东西）
    expect(row.className).toContain("text-destructive");
    const item = row.closest("div") as HTMLElement;
    expect(item.querySelector("svg.text-destructive")).toBeTruthy();
    // 顶部汇总图标不再涂绿（单项可以是绿的，所以按位置取头部那一行）
    const head = container.firstElementChild?.firstElementChild as HTMLElement;
    expect(head.querySelector("svg.text-emerald-500")).toBeNull();
  });

  it("探过且通了才画绿勾", () => {
    list(base({ channelOk: true, channelHint: null }));
    expect(screen.getByText("frida 通道（握手）")).toBeInTheDocument();
    expect(screen.getByTitle("frida 通道（握手）")).toBeInTheDocument();
  });
});
