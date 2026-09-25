import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import type { ConsoleEvent } from "./types";
import { EventRow } from "./FridaConsole";

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
