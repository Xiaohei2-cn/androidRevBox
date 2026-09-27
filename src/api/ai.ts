/**
 * 翻译接口的前端出口（UI-6 第二层的可选环节）。
 *
 * 三条纪律在后端落地（`services::ai_service`），这里只有一条配套约定：
 * **前端永远拿不到 API key 本体**——读配置只回 `hasApiKey` + `keyTail`（后 4 位），
 * 写配置时 key 是单向提交、不再回读。所以界面上"清空/替换"要让用户重新输一遍，
 * 而不是把已有值填回去（我们根本不知道它是什么）。
 */
import { invokeCommand } from "./client";

export interface AiConfig {
  baseUrl: string;
  model: string;
  enabled: boolean;
  hasApiKey: boolean;
  /** key 的后 4 位；没配是空串 */
  keyTail: string;
}

export interface TranslateOutcome {
  text: string;
  /** 原文超过上限被截断：译文只覆盖了送出去那一段 */
  sourceTruncated: boolean;
  /** 实际送出去的字符数 */
  sentChars: number;
}

export const aiApi = {
  getConfig(): Promise<AiConfig> {
    return invokeCommand<AiConfig>("ai_config_get");
  },
  /** `apiKey` 省略 = 保持现有 key 不动；空串 = 清掉 */
  setConfig(input: {
    baseUrl: string;
    model: string;
    enabled: boolean;
    apiKey?: string;
  }): Promise<AiConfig> {
    return invokeCommand<AiConfig>("ai_config_set", {
      args: {
        baseUrl: input.baseUrl,
        model: input.model,
        enabled: input.enabled,
        apiKey: input.apiKey === undefined ? null : input.apiKey,
      },
    });
  },
  /**
   * 把探测到的 help 文本翻成指定语言。
   *
   * ⚠️ 这条会把**待译文本发往用户自己配的接口**：只由用户点按钮触发，
   * 界面上必须把"原文会离开本机"说在前面。失败就保留原文，不阻塞任何事。
   */
  translate(text: string, targetLang: string, sourceLang?: string): Promise<TranslateOutcome> {
    return invokeCommand<TranslateOutcome>("ai_translate", {
      args: { text, targetLang, sourceLang: sourceLang ?? null },
    });
  },
};
