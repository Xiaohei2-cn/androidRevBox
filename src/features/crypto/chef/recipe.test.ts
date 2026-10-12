/**
 * 配方纯逻辑测试。
 * 这些是"看得见的行为"背后的算式：拖拽插入位置、停用步骤后的编号换算、
 * 粘贴配方的容错、localStorage 往返——界面测试很难稳定覆盖，这里直接钉死。
 */
import { beforeEach, describe, expect, it } from "vitest";

import type { ChefOperationDto } from "@/api/chef";
import {
  hydrateSteps,
  insertAt,
  loadSaved,
  loadWorkbench,
  moveBy,
  operationsById,
  parseRecipe,
  removeAt,
  serializeRecipe,
  stepFromOperation,
  toBakeRequest,
  visibleIndexOf,
} from "./recipe";

const base64: ChefOperationDto = {
  id: "base64Encode",
  category: "encoding",
  args: [
    { name: "urlSafe", kind: "bool", default: "false", options: [] },
    { name: "noWrap", kind: "bool", default: "true", options: [] },
  ],
};
const md5: ChefOperationDto = {
  id: "md5",
  category: "digest",
  args: [{ name: "uppercase", kind: "bool", default: "false", options: [] }],
};
const rot13: ChefOperationDto = { id: "rot13", category: "cipher", args: [] };
const list = [base64, md5, rot13];
const byId = operationsById(list);

beforeEach(() => {
  localStorage.clear();
});

describe("步骤装配", () => {
  it("新步骤带全默认参数，key 互不相同", () => {
    const a = stepFromOperation(base64);
    const b = stepFromOperation(base64);
    expect(a.args).toEqual({ urlSafe: "false", noWrap: "true" });
    expect(a.key).not.toBe(b.key);
    expect(a.disabled).toBe(false);
  });

  it("无参数算法也有空 args 对象（后端按缺省补齐）", () => {
    expect(stepFromOperation(rot13).args).toEqual({});
  });
});

describe("顺序操作", () => {
  const steps = [stepFromOperation(base64), stepFromOperation(md5), stepFromOperation(rot13)];

  it("insertAt 越界会被钳制而不是抛错", () => {
    expect(insertAt(steps, -1, steps[2]).map((s) => s.id)).toEqual([
      steps[2].id,
      "base64Encode",
      "md5",
      "rot13",
    ]);
    expect(insertAt(steps, 99, steps[0]).length).toBe(4);
  });

  it("moveBy 到头/到尾保持原样", () => {
    expect(moveBy(steps, 0, -1)).toBe(steps);
    expect(moveBy(steps, 2, 1)).toBe(steps);
    expect(moveBy(steps, 0, 1).map((s) => s.id)).toEqual(["md5", "base64Encode", "rot13"]);
  });

  it("removeAt 只删指定下标", () => {
    expect(removeAt(steps, 1).map((s) => s.id)).toEqual(["base64Encode", "rot13"]);
  });
});

describe("交给后端的请求", () => {
  it("被停用的步骤不进配方，但界面上的编号仍能换算回原下标", () => {
    const steps = [stepFromOperation(base64), stepFromOperation(md5), stepFromOperation(rot13)];
    steps[1].disabled = true;
    expect(toBakeRequest("hi", "utf8", steps).recipe.map((s) => s.id)).toEqual([
      "base64Encode",
      "rot13",
    ]);
    expect(visibleIndexOf(steps, 0)).toBe(0);
    expect(visibleIndexOf(steps, 1)).toBe(2);
    expect(visibleIndexOf(steps, 2)).toBe(-1);
  });

  it("参数是拷贝，改界面不会串到上一次请求", () => {
    const steps = [stepFromOperation(base64)];
    const request = toBakeRequest("hi", "utf8", steps);
    steps[0].args.urlSafe = "true";
    expect(request.recipe[0].args.urlSafe).toBe("false");
  });
});

describe("配方文本往返", () => {
  it("serialize 出来可以直接 parse 回去（复制配方给同事）", () => {
    const steps = [stepFromOperation(base64)];
    steps[0].args.noWrap = "false";
    const parsed = parseRecipe(serializeRecipe(steps), byId);
    expect(parsed.rejected).toBe(0);
    expect(parsed.steps[0].args).toEqual({ urlSafe: "false", noWrap: "false" });
  });

  it("未知算法静默丢弃并回报数量；也接受裸字符串数组与 {recipe:[…]} 形状", () => {
    const loose = parseRecipe('["md5","nope"]', byId);
    expect(loose.steps.map((s) => s.id)).toEqual(["md5"]);
    expect(loose.rejected).toBe(1);
    const wrapped = parseRecipe('{"recipe":[{"id":"rot13"}]}', byId);
    expect(wrapped.steps.map((s) => s.id)).toEqual(["rot13"]);
    expect(parseRecipe("不是 JSON", byId)).toEqual({ steps: [], rejected: 0 });
  });

  it("粘回来的数字/布尔参数统一成字符串（后端只认字符串）", () => {
    const parsed = parseRecipe('[{"id":"pbkdf2","args":{"iterations":12}}]', byId);
    expect(parsed.steps).toEqual([]);
    const known = parseRecipe('[{"id":"md5","args":{"uppercase":true}}]', byId);
    expect(known.steps[0].args.uppercase).toBe("true");
  });

  it("词典外的参数一律忽略，避免把脏数据喂给后端", () => {
    const parsed = parseRecipe('[{"id":"md5","args":{"evil":"x"}}]', byId);
    expect(parsed.steps[0].args).toEqual({ uppercase: "false" });
  });
});

describe("本地持久化", () => {
  it("缺键/坏 JSON 都当成首次使用，不抛错", () => {
    expect(loadWorkbench()).toBeNull();
    localStorage.setItem("chef.workbench.v1", "{坏数据");
    expect(loadWorkbench()).toBeNull();
    expect(loadSaved()).toEqual([]);
  });

  it("hydrateSteps 用注册表补齐缺失参数、丢掉已删除的算法", () => {
    const steps = hydrateSteps(
      [
        { id: "base64Encode", args: { urlSafe: "true" }, disabled: false },
        { id: "goneOperation", args: {}, disabled: false },
      ],
      byId,
    );
    expect(steps.length).toBe(1);
    expect(steps[0].args).toEqual({ urlSafe: "true", noWrap: "true" });
  });
});
