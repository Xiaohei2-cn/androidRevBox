import { SubTabs } from "@/components/nav/SubTabs";
import { SoReplacePage } from "@/features/binary/SoReplacePage";

/** 二进制页：so 替换（修补后的 .so 写回安装目录，免重打包） */
export function BinaryPage() {
  return (
    <SubTabs
      tabs={[
        {
          id: "so-replace",
          label: "so 替换",
          content: <SoReplacePage />,
        },
      ]}
    />
  );
}
