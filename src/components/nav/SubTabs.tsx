import { useEffect, useState, type ReactNode } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export interface SubTabDef {
  id: string;
  label: string;
  content: ReactNode;
}

interface SubTabsProps {
  tabs: SubTabDef[];
  /** 受控当前 tab（与 onValueChange 成对使用）；缺省为非受控 */
  value?: string;
  onValueChange?: (id: string) => void;
  /** 外部要求切到的 tab（变化即切换）；受控/非受控均可 */
  activateSignal?: string | null;
}

/**
 * 页内分 tab（子导航）：每个总 tab 的内容区顶部横向排布。
 * P0 建立通用机制；P8 增加受控模式与 activateSignal（「去配置」跨 tab 定位）。
 * 内容保持挂载（inactive 仅 hidden），跨 tab 的聚焦/定位与查询状态不丢。
 */
export function SubTabs({ tabs, value, onValueChange, activateSignal }: SubTabsProps) {
  const [inner, setInner] = useState(tabs[0]?.id ?? "");
  const current = value ?? inner;
  const setCurrent = (id: string) => {
    setInner(id);
    onValueChange?.(id);
  };

  useEffect(() => {
    if (activateSignal && tabs.some((tb) => tb.id === activateSignal)) {
      setCurrent(activateSignal);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activateSignal]);

  if (tabs.length === 0) return null;
  return (
    <Tabs value={current} onValueChange={setCurrent} className="flex h-full flex-col gap-3">
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
          forceMount
          className="min-h-0 flex-1 overflow-auto data-[state=inactive]:hidden"
        >
          {tab.content}
        </TabsContent>
      ))}
    </Tabs>
  );
}
