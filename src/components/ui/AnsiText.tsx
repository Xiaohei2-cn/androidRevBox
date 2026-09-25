import { Fragment } from "react";

import { ansiRuns } from "@/lib/ansi";
import { cn } from "@/lib/utils";

/**
 * 渲染带 ANSI 颜色的控制台文本：SGR → 样式，其它转义丢掉。
 *
 * 不额外包一层元素（返回片段），这样父级的 `whitespace-pre-wrap`、字号与
 * 三色轨道的颜色都照常生效——hexdump 的列对齐靠空格，父级的 pre 语义不能被破坏。
 */
export function AnsiText({ text }: { text: string }) {
  const runs = ansiRuns(text);
  return (
    <>
      {runs.map((run, i) =>
        run.color || run.bold ? (
          <Fragment key={i}>
            <span className={cn(run.color, run.bold && "font-semibold")}>{run.text}</span>
          </Fragment>
        ) : (
          <Fragment key={i}>{run.text}</Fragment>
        ),
      )}
    </>
  );
}
