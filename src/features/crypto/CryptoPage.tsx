import { SubTabs } from "@/components/nav/SubTabs";
import { PageHeader } from "@/components/ui/page-header";
import { Workbench } from "@/features/crypto/chef/Workbench";
import { useI18n } from "@/i18n";

/**
 * 算法中心（原「算法工具」）。
 *
 * 第一个子 tab 是算法合集：CyberChef 网页版的三栏布局（清单 / 组合 / 输入输出），
 * 算法本体已全部移植成 Rust（`src-tauri/src/services/chef/`），界面只负责编排与展示。
 * 后面的子 tab 留给「按业务预置的一键工具」（如 sign 还原、抓包解链）——
 * 那些是把合集里的算法固定成一条配方再包一层，不该塞进合集本身。
 */
export function CryptoPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.crypto")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          tabs={[
            { id: "collection", label: t("chef.collection"), content: <Workbench /> },
            { id: "sign", label: t("chef.signRules"), content: <SignRulesPlaceholder /> },
          ]}
        />
      </div>
    </div>
  );
}

function SignRulesPlaceholder() {
  const { t } = useI18n();
  return (
    <div className="flex h-full items-center justify-center px-6 text-center">
      <p className="max-w-md text-xs leading-relaxed text-muted-foreground">
        {t("chef.signRulesHint")}
      </p>
    </div>
  );
}
