/**
 * 气泡行的两种排布必须分得开。
 *
 * 真实事故：`multiline` 名字读起来像"这条内容有多行"，于是有人拿它装多个纵向块，
 * 而它只改纵向对齐、容器仍是**横向** flex——每个块变成 row 的一个子项、被压到最小宽度，
 * 中文逐字换行成竖排（二进制托管页的「这一次是用这些参数起的：…」整列竖着）。
 *
 * jsdom 不编译 Tailwind，拿不到 computedStyle，所以这里断言的是**类名契约**：
 * 堆叠必须出现 `flex-col`，横向行必须不出现。这条测试的价值不在覆盖样式，而在
 * 有人再把纵向块塞进 `multiline` 时会被它拦下（配合 `stack` 的文档）。
 */
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { List, ListRow } from "./list";

function contentOf(testId: string): HTMLElement {
  const li = document.querySelector(`[data-testid="${testId}"]`);
  expect(li).not.toBeNull();
  // 内容容器是 li 的第一个元素（无论可点与否都包一层）
  const container = li!.firstElementChild as HTMLElement;
  expect(container).not.toBeNull();
  return container;
}

describe("ListRow 排布", () => {
  it("默认是一行：横向 flex，纵向居中", () => {
    render(
      <List>
        <ListRow testId="row-default">
          <span>a</span>
          <span>b</span>
        </ListRow>
      </List>,
    );
    const container = contentOf("row-default");
    expect(container.className).toContain("flex");
    expect(container.className).not.toContain("flex-col");
    expect(container.className).toContain("items-center");
  });

  it("multiline 只改纵向对齐，方向仍是横向", () => {
    render(
      <List>
        <ListRow testId="row-multiline" multiline>
          <span>✓</span>
          <span>步骤名</span>
          <span>可能换行的详情</span>
        </ListRow>
      </List>,
    );
    const container = contentOf("row-multiline");
    expect(container.className).toContain("items-start");
    expect(container.className).not.toContain("flex-col");
  });

  it("stack 才是纵向堆叠：子元素上下排列、各自占满整行", () => {
    render(
      <List>
        <ListRow testId="row-stack" stack>
          <div>文件名</div>
          <div>动作</div>
          <div>参数</div>
        </ListRow>
      </List>,
    );
    const container = contentOf("row-stack");
    expect(container.className).toContain("flex-col");
    expect(container.className).toContain("items-stretch");
    expect(container.className).not.toContain("items-center");
    // 堆叠时 gap 归零：各块自带 mt-*，再叠 gap 会比改动前更松
    expect(container.className).toContain("gap-0");
  });

  it("stack 与可点行一起用也只排一列（button 版本同样生效）", () => {
    render(
      <List>
        <ListRow testId="row-stack-clickable" stack onClick={() => undefined}>
          <div>第一块</div>
          <div>第二块</div>
        </ListRow>
      </List>,
    );
    const button = document.querySelector(
      '[data-testid="row-stack-clickable"] button',
    ) as HTMLElement;
    expect(button).not.toBeNull();
    expect(button.className).toContain("flex-col");
  });
});
