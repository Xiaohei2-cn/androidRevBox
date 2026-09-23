import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { I18nProvider } from "@/i18n/I18nProvider";
import { PathBar } from "./PathBar";
import { usePathHistory } from "./usePathHistory";

/**
 * 地址栏的渲染级验证：三个按钮的**可用性**、"手敲要回车才生效"、以及换设备时清空历史。
 * 纯函数那层（pathHistory.test.ts）测的是规则，这里测的是接线。
 */
function Harness({ onPath, resetKey }: { onPath: (path: string) => void; resetKey?: string | null }) {
  const nav = usePathHistory("/sdcard", resetKey ?? null);
  onPath(nav.path);
  return (
    <I18nProvider>
      <PathBar nav={nav} onRefresh={() => undefined} />
    </I18nProvider>
  );
}

let visited: string[] = [];
const collect = (path: string) => visited.push(path);

const back = () => screen.getByRole("button", { name: "后退" });
const forward = () => screen.getByRole("button", { name: "前进" });
const up = () => screen.getByRole("button", { name: "上一级" });
const input = () => screen.getByRole("textbox", { name: "设备上的路径，如 /sdcard/Download" });
const goButton = () => screen.getByRole("button", { name: "进入" });
const at = () => visited[visited.length - 1];

beforeEach(() => {
  visited = [];
  localStorage.setItem("app.settings.locale", "zh-CN");
});

describe("文件页地址栏", () => {
  it("起点：没有历史可退，也没有前进分支，但从 /sdcard 可以上一级", () => {
    render(<Harness onPath={collect} />);
    expect(back()).toBeDisabled();
    expect(forward()).toBeDisabled();
    expect(up()).toBeEnabled();
  });

  it("只在回车时才切目录；此后后退可用、前进能再回来", () => {
    render(<Harness onPath={collect} />);
    fireEvent.change(input(), { target: { value: "/data/local/tmp" } });
    // 还没回车：路径没变，也不该留下浏览记录
    expect(at()).toBe("/sdcard");
    expect(back()).toBeDisabled();
    expect(goButton()).toBeEnabled();

    fireEvent.keyDown(input(), { key: "Enter" });
    expect(at()).toBe("/data/local/tmp");
    expect(back()).toBeEnabled();
    expect(forward()).toBeDisabled();

    fireEvent.click(back());
    expect(at()).toBe("/sdcard");
    expect(forward()).toBeEnabled();

    fireEvent.click(forward());
    expect(at()).toBe("/data/local/tmp");
  });

  it("上一级按路径结构走；已经在根时按钮禁用，而不是拼出 /..", () => {
    render(<Harness onPath={collect} />);
    fireEvent.click(up());
    expect(at()).toBe("/");
    expect(up()).toBeDisabled();
    expect(back()).toBeEnabled();

    fireEvent.click(back());
    expect(at()).toBe("/sdcard");
    expect(up()).toBeEnabled();
  });

  it("换设备时清空历史，不留在另一台机的路径上", () => {
    const view = render(<Harness onPath={collect} resetKey="SERIAL-A" />);
    fireEvent.click(up());
    expect(at()).toBe("/");
    expect(back()).toBeEnabled();

    view.rerender(<Harness onPath={collect} resetKey="SERIAL-B" />);
    expect(at()).toBe("/sdcard");
    expect(back()).toBeDisabled();
    expect(forward()).toBeDisabled();
  });
});
