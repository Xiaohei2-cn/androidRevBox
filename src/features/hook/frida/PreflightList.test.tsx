import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { PreflightDto } from "@/api/hook";
import { PreflightList } from "./SettingsPanel";

const T = (k: string) => k === "hook.frida.channel" ? "frida 通道（握手）" : k === "hook.frida.gotoForward" ? "去转发" : k === "hook.frida.fix" ? "去修" : k;

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
    const label = screen.getByText("frida 通道（握手）");
    const pill = label.closest("button") as HTMLElement;
    expect(pill).toBeTruthy();
    // 原因挂在药丸的 title 上：版本不匹配那句话很长，截断后也要能查到
    expect(pill.getAttribute("title")).toBe(hint);
    // 这一项自己必须是红的
    expect(pill.className).toContain("text-destructive");
    expect(pill.querySelector("svg.text-destructive")).toBeTruthy();
    // 而且必须有"去哪修"的动作：remote 类跳端口转发页
    // 失败且知道去哪修的，药丸上直接带动作（这一项的去处是托管二进制页）
    expect(pill.textContent).toContain("去修");
    expect(pill).toBeEnabled();
    // 汇总图标压根不该存在：单项有绿有红时，一个总体绿勾就是撒谎
    expect(container.querySelector('[data-testid="preflight-summary"]')).toBeNull();
    expect((container.firstElementChild as HTMLElement).tagName).toBe("DIV");
  });

  it("探过且通了才画绿勾", () => {
    list(base({ channelOk: true, channelHint: null }));
    expect(screen.getByText("frida 通道（握手）")).toBeInTheDocument();
    expect(screen.getByTitle("frida 通道（握手）")).toBeInTheDocument();
  });
});
