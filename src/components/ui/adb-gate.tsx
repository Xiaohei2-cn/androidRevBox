import { EmptyState } from "@/components/ui/empty-state";
import { Usb } from "lucide-react";

/**
 * adb 未就绪门控（UI 统一改版）：ADB/二进制/脱壳等依赖设备链路的子页共用。
 * - 探测中（无 hint）：骨架屏，替代旧版「Checking…」一行小字悬在空白中央；
 * - 环境缺失（有 hint）：空态卡说明原因，替代旧版无上下文的单行提示。
 */
export function AdbNotReadyState({ hint }: { hint?: string | null }) {
  if (!hint) {
    return (
      <div className="flex h-full flex-col gap-3">
        <div className="skeleton h-8 w-64" />
        <div className="skeleton h-14" />
        <div className="skeleton h-14" />
        <div className="skeleton h-14" />
        <div className="skeleton h-14 w-2/3" />
      </div>
    );
  }
  return (
    <EmptyState
      icon={<Usb className="h-5 w-5" />}
      title="设备链路未就绪"
      description={hint}
    />
  );
}
