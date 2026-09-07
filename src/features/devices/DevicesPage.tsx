import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";

/** 设备页：分 tab 机制验证页之一，内容在 P3 ADB 阶段实现 */
export function DevicesPage() {
  return (
    <SubTabs
      tabs={[
        {
          id: "list",
          label: "设备列表",
          content: (
            <Placeholder
              title="ADB 设备列表"
              description="设备发现、属性、Shell、应用管理将在此实现"
              phase="P3 · ADB 能力"
            />
          ),
        },
        {
          id: "files",
          label: "文件",
          content: (
            <Placeholder
              title="设备文件浏览"
              description="push / pull / ls / stat 等文件操作将在此实现"
              phase="P3 · ADB 能力"
            />
          ),
        },
        {
          id: "logcat",
          label: "Logcat",
          content: (
            <Placeholder
              title="Logcat 日志流"
              description="实时 logcat 与过滤将在此实现"
              phase="P3 · ADB 能力"
            />
          ),
        },
      ]}
    />
  );
}
