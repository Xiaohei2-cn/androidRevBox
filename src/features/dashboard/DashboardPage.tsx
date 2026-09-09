import { useQuery } from "@tanstack/react-query";
import { envApi } from "@/api/env";
import { AdbCard } from "./AdbCard";
import { FridaCard, McpCard, NodeCard, PythonCard } from "./EnvCards";
import { ForegroundCard } from "./ForegroundCard";

/**
 * 仪表盘（P7）：环境/工具探测中心。
 * 布局契约（PHASES §10）：左右两列网格；任何卡片不得独占一行；
 * 唯一例外「安卓前台应用」整行且排最底。卡片顺序 = 先系统、再环境、再工具、安卓前台最后。
 * 系统状态四卡（后端连接/应用版本/Tauri 版本/平台）已迁至 设置 → 关于。
 */
export function DashboardPage() {
  return (
    <div
      data-testid="dashboard-grid"
      className="grid h-full grid-cols-2 content-start gap-4 overflow-auto"
    >
      <AdbCard />
      <PythonCard />
      <NodeCard />
      <FridaCardWrapper />
      <McpCard testid="env-ida" title="IDA MCP" queryFn={envApi.idaMcp} />
      <McpCard testid="env-jadx" title="jadx-gui MCP" queryFn={envApi.jadxMcp} />
      {/* 唯一整行卡：安卓前台应用，置于最底 */}
      <ForegroundCard />
    </div>
  );
}

/** Frida 卡依赖 Python 就绪：先探 Python，ready 才启用 Frida 查询（§10 剪枝） */
function FridaCardWrapper() {
  const { data: python } = useQuery({
    queryKey: ["env", "python"],
    queryFn: envApi.python,
    staleTime: 30_000,
    retry: false,
  });
  return <FridaCard pythonReady={python?.ready} />;
}
