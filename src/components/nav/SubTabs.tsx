import type { ReactNode } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export interface SubTabDef {
  id: string;
  label: string;
  content: ReactNode;
}

/**
 * 页内分 tab（子导航）：每个总 tab 的内容区顶部横向排布。
 * P0 建立通用机制，业务阶段往里填真实内容。
 */
export function SubTabs({ tabs }: { tabs: SubTabDef[] }) {
  if (tabs.length === 0) return null;
  return (
    <Tabs defaultValue={tabs[0].id} className="flex h-full flex-col gap-3">
      <TabsList className="w-fit shrink-0">
        {tabs.map((tab) => (
          <TabsTrigger key={tab.id} value={tab.id}>
            {tab.label}
          </TabsTrigger>
        ))}
      </TabsList>
      {tabs.map((tab) => (
        <TabsContent
          key={tab.id}
          value={tab.id}
          className="min-h-0 flex-1 overflow-auto data-[state=inactive]:hidden"
        >
          {tab.content}
        </TabsContent>
      ))}
    </Tabs>
  );
}
