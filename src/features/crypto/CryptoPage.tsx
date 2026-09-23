import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { useI18n } from "@/i18n";

/** 算法工具页：分 tab 机制验证页之一，算法插件在 P5 阶段接入 */
export function CryptoPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.crypto")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          tabs={[
            {
              id: "encoding",
              label: "编码 / 哈希",
              content: (
                <Placeholder
                  title="编码与哈希"
                  description="Base64 / Hex / MD5 / SHA / SM3 等将在此实现"
                  phase="P5 · 算法中心"
                />
              ),
            },
            {
              id: "cipher",
              label: "加解密",
              content: (
                <Placeholder
                  title="加解密算法"
                  description="AES / SM4 / RSA / HMAC 等将在此实现"
                  phase="P5 · 算法中心"
                />
              ),
            },
          ]}
        />
      </div>
    </div>
  );
}
