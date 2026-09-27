import { beforeEach, describe, expect, it } from "vitest";

import { loadRootPref, rootPrefKey, saveRootPref, shouldTryRoot } from "./hostedRoot";

describe("托管 Root 偏好（默认想开，但要现场证明）", () => {
  beforeEach(() => localStorage.clear());

  it("没存过 = unset（进页面要探测）", () => {
    expect(loadRootPref(null)).toBe("unset");
    expect(loadRootPref("PIXEL-1")).toBe("unset");
    expect(shouldTryRoot("unset")).toBe(true);
  });

  it("手动关过的设备不再自动开，也不再探测", () => {
    saveRootPref("PIXEL-1", false);
    expect(localStorage.getItem(rootPrefKey("PIXEL-1"))).toBe("0");
    expect(loadRootPref("PIXEL-1")).toBe("off");
    expect(shouldTryRoot("off")).toBe(false);
  });

  it("手动开过 = 继续尝试开", () => {
    saveRootPref("PIXEL-1", true);
    expect(loadRootPref("PIXEL-1")).toBe("on");
    expect(shouldTryRoot("on")).toBe(true);
  });

  it("脏值落回 unset（不当成 off 静默关掉用户的 root）", () => {
    localStorage.setItem(rootPrefKey("PIXEL-1"), "maybe");
    expect(loadRootPref("PIXEL-1")).toBe("unset");
  });

  it("偏好按设备分开存", () => {
    saveRootPref("A", false);
    saveRootPref("B", true);
    expect(loadRootPref("A")).toBe("off");
    expect(loadRootPref("B")).toBe("on");
  });
});
