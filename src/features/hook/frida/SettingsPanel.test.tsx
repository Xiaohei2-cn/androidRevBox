/**
 * A 区那两个分段选择（连接 / 运行模式）的选中态（第九十三轮用户要求：选中＝蓝）。
 *
 * 之前选中的那颗是 `bg-background`（浅底 + 影子），在深色主题下几乎看不出"哪个是当前值"；
 * 现在走主色 `bg-primary text-primary-foreground`——就是设置页分段选择、左侧 tab 齿块、
 * 「启动」按钮共用的那一个蓝，不再新造一种"选中"的语言。
 *
 * 判据刻意落在语义（aria-checked）+ 类名（主色只挂选中的那颗）上：
 * jsdom 没有排版，量不到像素，能钉的是"我们交给样式的是哪一套语言"。
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Segmented } from "./SettingsPanel";

const OPTIONS = [
  { v: "usb", label: "USB" },
  { v: "remote", label: "远程端口" },
];

function paintOf(name: string) {
  return screen.getByRole("radio", { name }).className;
}

describe("A 区分段选择的选中态", () => {
  it("选中的那颗是主色，另一颗不是", () => {
    render(<Segmented value="usb" options={OPTIONS} onChange={vi.fn()} />);
    expect(paintOf("USB")).toContain("bg-primary");
    expect(paintOf("USB")).toContain("text-primary-foreground");
    expect(paintOf("USB")).not.toContain("bg-background");
    expect(paintOf("远程端口")).not.toContain("bg-primary");
    expect(screen.getByRole("radio", { name: "USB" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "远程端口" })).toHaveAttribute("aria-checked", "false");
  });

  it("换值之后主色跟着走（不是写死在第一颗上）", () => {
    const onChange = vi.fn();
    const { rerender } = render(<Segmented value="usb" options={OPTIONS} onChange={onChange} />);
    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    expect(onChange).toHaveBeenCalledWith("remote");

    rerender(<Segmented value="remote" options={OPTIONS} onChange={onChange} />);
    expect(paintOf("远程端口")).toContain("bg-primary");
    expect(paintOf("USB")).not.toContain("bg-primary");
  });

  it("会话跑起来了就整组禁用：选中样式照旧，点不动", () => {
    const onChange = vi.fn();
    render(<Segmented value="usb" options={OPTIONS} onChange={onChange} disabled />);
    expect(paintOf("USB")).toContain("bg-primary");
    for (const name of ["USB", "远程端口"]) {
      expect(screen.getByRole("radio", { name })).toBeDisabled();
    }
    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    expect(onChange).not.toHaveBeenCalled();
  });
});
