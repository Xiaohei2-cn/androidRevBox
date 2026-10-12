import { invokeCommand } from "./client";

/**
 * 算法中心（CyberChef 移植）的 IPC 封装。
 * 形状与 src-tauri/src/models/chef.rs 逐字段对齐（camelCase）。
 */

/** 一个参数控件：kind 决定界面用什么控件 */
export interface ChefArgDto {
  name: string;
  /** text | bool | number | choice */
  kind: string;
  default: string;
  options: string[];
}

export interface ChefOperationDto {
  id: string;
  /** 分类 id；界面用 chef.category.<id> 出名字 */
  category: string;
  args: ChefArgDto[];
}

/** 配方里的一步（参数一律字符串；复选框是 "true"/"false"） */
export interface ChefStepInput {
  id: string;
  args: Record<string, string>;
}

export interface ChefBakeRequest {
  input: string;
  /** utf8 | hex | base64 | base64url | decimal */
  inputEncoding?: string;
  recipe: ChefStepInput[];
}

export interface ChefStepResult {
  id: string;
  ok: boolean;
  bytes: number;
  /** 该步输出是否仍是合法 UTF-8（不是的话界面上会提醒「从这一步开始是二进制」） */
  utf8: boolean;
  preview: string;
  error?: string | null;
}

export interface ChefBakeResult {
  ok: boolean;
  output: string;
  outputBytes: number;
  outputUtf8: boolean;
  outputHex: string;
  outputBase64: string;
  steps: ChefStepResult[];
  /** 失败发生在第几步（0 起）；成功为 null */
  failedAt?: number | null;
  inputBytes: number;
}

/** 输入框可选的"怎么把这段文本变成字节" */
export const INPUT_ENCODINGS = ["utf8", "hex", "base64", "base64url", "decimal"] as const;
/** 输出面板的展示口径（只影响显示，不重跑配方） */
export const OUTPUT_DISPLAYS = ["text", "hex", "base64"] as const;

export const chefApi = {
  /** 算法清单（id / 分类 / 参数表）；文案在前端词典里 */
  operations(): Promise<ChefOperationDto[]> {
    return invokeCommand<ChefOperationDto[]>("chef_operations");
  },
  /** 一次交整条配方：中间步骤是二进制，逐步 IPC 会在非 UTF-8 处丢字节 */
  bake(request: ChefBakeRequest): Promise<ChefBakeResult> {
    return invokeCommand<ChefBakeResult>("chef_bake", { request });
  },
};
