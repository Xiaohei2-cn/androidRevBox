import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";

/** 算法工具页：分 tab 机制验证页之一，算法插件在 P5 阶段接入 */
export function CryptoPage() {
  return (
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
  );
}
