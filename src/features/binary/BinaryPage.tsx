import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";

/** 二进制页占位：静态分析、patch、符号/字符串工具将在后续阶段实现 */
export function BinaryPage() {
  return (
    <SubTabs
      tabs={[
        {
          id: "analyze",
          label: "分析",
          content: (
            <Placeholder
              title="二进制分析"
              description="so/apk 静态分析、字符串与符号浏览、patch 工具将在此实现"
              phase="占位 · 规划中"
            />
          ),
        },
      ]}
    />
  );
}
