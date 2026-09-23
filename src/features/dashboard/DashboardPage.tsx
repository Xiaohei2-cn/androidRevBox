import { useQuery } from "@tanstack/react-query";
import { envApi } from "@/api/env";
import { useI18n } from "@/i18n";
import { PageHeader } from "@/components/ui/page-header";
import { AdbCard } from "./AdbCard";
import { FridaCard, McpCard, NodeCard, PythonCard } from "./EnvCards";

/**
 * 仪表盘（P7）：环境/工具探测中心。
 * 布局契约（PHASES §10）：左右两列网格；任何卡片不得独占一行；
 * 唯一例外「安卓前台应用」整行且排最底。卡片顺序 = 先系统、再环境、再工具、安卓前台最后。
 * 系统状态四卡（后端连接/应用版本/Tauri 版本/平台）已迁至 设置 → 关于。
 * UI 统一改版：顶部加页头锚点；卡片等高靠 grid 行拉伸 + 卡内 flex 撑满。
 */
export function DashboardPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader
        title={t("nav.dashboard")}
        description={t("dashboard.header.desc")}
      />
      <div
        data-testid="dashboard-grid"
        className="grid min-h-0 flex-1 grid-cols-2 content-start gap-4 overflow-auto pb-1"
      >
        <AdbCard />
        <PythonCard />
        <NodeCard />
        <FridaCardWrapper />
        <McpCard
          testid="env-ida"
          title="IDA"
          queryFn={envApi.idaMcp}
          configKey="app.tools.ida_mcp_port"
          brand="ida"
          appLabel="IDA"
        />
        <McpCard
          testid="env-jadx"
          title="jadx-gui"
          queryFn={envApi.jadxMcp}
          configKey="app.tools.jadx_mcp_port"
          brand="jadx"
          appLabel="jadx-gui"
        />
      </div>
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
