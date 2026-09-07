import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, XCircle } from "lucide-react";
import { systemApi } from "@/api/system";

/** 仪表盘：P0 用 system_ping 验证前端 → Tauri Command 通路 */
export function DashboardPage() {
  const { data, isLoading, error } = useQuery({
    queryKey: ["system", "ping"],
    queryFn: systemApi.ping,
  });

  return (
    <div className="flex h-full flex-col gap-4">
      <div className="grid grid-cols-2 gap-4 xl:grid-cols-4">
        <StatusCard
          title="后端连接"
          loading={isLoading}
          ok={!error}
          value={error ? "未连接" : "正常"}
        />
        <InfoCard title="应用版本" value={data?.appVersion ?? "—"} />
        <InfoCard title="Tauri 版本" value={data?.tauriVersion ?? "—"} />
        <InfoCard
          title="运行平台"
          value={data ? `${data.os} · ${data.arch}` : "—"}
        />
      </div>
      <div className="rounded-xl border border-dashed p-6 text-center text-xs text-muted-foreground">
        最近命令 / 常用工具 / 插件状态等聚合信息将在 P7 UX 阶段补全
      </div>
    </div>
  );
}

function StatusCard({
  title,
  value,
  ok,
  loading,
}: {
  title: string;
  value: string;
  ok: boolean;
  loading: boolean;
}) {
  return (
    <div className="rounded-xl border bg-card p-4">
      <p className="text-xs text-muted-foreground">{title}</p>
      <div className="mt-2 flex items-center gap-1.5 text-lg font-semibold">
        {loading ? (
          <span className="text-sm text-muted-foreground">检测中…</span>
        ) : (
          <>
            {ok ? (
              <CheckCircle2 className="h-4 w-4 text-emerald-500" />
            ) : (
              <XCircle className="h-4 w-4 text-red-500" />
            )}
            {value}
          </>
        )}
      </div>
    </div>
  );
}

function InfoCard({ title, value }: { title: string; value: string }) {
  return (
    <div className="rounded-xl border bg-card p-4">
      <p className="text-xs text-muted-foreground">{title}</p>
      <p className="mt-2 text-lg font-semibold">{value}</p>
    </div>
  );
}
