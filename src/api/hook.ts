import { invokeCommand } from "./client";

/** 与 Rust hook_service DTO 对齐（camelCase）；协议解析在前端（设计 §5.2/§6.3） */

export interface JsFileDto {
  name: string;
  path: string;
  size: number;
  /** Unix 秒 */
  mtime: number;
}

export interface PreflightDto {
  adbOk: boolean;
  adbHint?: string | null;
  pythonOk: boolean;
  pythonHint?: string | null;
  pythonPath?: string | null;
  /**
   * 官方 REPL（`python -m frida_tools.repl`）对这条 Python 的要求：**frida-tools 在不在**。
   * 与 `lib/fridaSession` 的 `no-frida-tools` 判据同源——前置亮绿与实际点下去报不报
   * `No module named frida_tools` 必须是同一件事。
   */
  fridaOk: boolean;
  /** Python 绑定包（`import frida`）版本；只有 frida-tools 的机器上为 null */
  fridaVersion?: string | null;
  /** frida-tools 版本（REPL 本体）；两版都给出来后置药丸显示得更准确 */
  fridaToolsVersion?: string | null;
  fridaHint?: string | null;
  /** 远程模式探活结果；非远程模式 null（不探） */
  remoteOk: boolean | null;
  remoteHint?: string | null;
  /** 与设备 frida-server 的真实握手；null/undefined = 没探（Python 未就绪），界面上不出现这一项 */
  channelOk?: boolean | null;
  channelHint?: string | null;
}

/** 从用户指定的项目目录里读到的 frida 类型库（智能提示来源） */
export interface FridaTypesDto {
  /** @types/frida-gum/index.d.ts 正文 */
  gum: string;
  gumVersion?: string | null;
  /** gum 类型库是否自带 `Java` 命名空间（18.x 及更早自带；自带就不该再注 java-bridge） */
  gumHasJava: boolean;
  /** frida-java-bridge/index.d.ts 正文；该目录里没有则为 null */
  javaBridge?: string | null;
  javaBridgeVersion?: string | null;
  gumPath: string;
  javaBridgePath?: string | null;
}

/** 自动发现到的一个 frida 类型库位置（设置里的候选） */
export interface FridaTypeDir {
  projectRoot: string;
  version?: string | null;
  mtime: number;
}

export const hookApi = {
  /** 扫工作目录一层 *.js；dir 缺省用配置键 app.hook.workdir */
  jsList(dir?: string): Promise<JsFileDto[]> {
    return invokeCommand<JsFileDto[]>("hook_js_list", { dir: dir ?? null });
  },
  /**
   * 前置检查链聚合（adb/python/frida-tools/远程 TCP/frida 通道握手）。
   * serial 只在 USB 模式下给：握手必须连**要用的那台设备**，不然探的是别机的 server。
   */
  preflight(remote?: string, serial?: string): Promise<PreflightDto> {
    return invokeCommand<PreflightDto>("hook_preflight", {
      remote: remote ?? null,
      serial: serial ?? null,
    });
  },
  /**
   * 读工作目录里的一个脚本正文（IDE 打开脚本时用）。
   * 边界（名字白名单、一层目录内、拒绝符号链接）全在宿主侧，前端只传名字。
   */
  scriptRead(name: string): Promise<string> {
    return invokeCommand<string>("hook_script_read", { name });
  },
  /** 写一个脚本（IDE 保存；返回落盘字节数，宿主写完回读自证）。
   * createOnly=true 是「新建」语义：撞名拒绝，不静默覆盖已有脚本 */
  scriptWrite(name: string, content: string, createOnly = false): Promise<number> {
    return invokeCommand<number>("hook_script_write", { name, content, createOnly });
  },
  /** 删一个脚本（IDE 的「删除」；只删工作目录里的普通 .js） */
  scriptDelete(name: string): Promise<void> {
    return invokeCommand<void>("hook_script_delete", { name });
  },
  /**
   * 读用户指定的**项目目录**里的 frida 类型库（IDE 智能提示的"提示内容"来源）。
   * dir 缺省用配置键 app.hook.frida_types_dir。找不到那份 @types/frida-gum 会抛错，
   * 由调用方回退到"按 frida 版本从 CDN 取"。
   */
  fridaTypes(dir?: string): Promise<FridaTypesDto> {
    return invokeCommand<FridaTypesDto>("hook_frida_types", { dir: dir ?? null });
  },
  /**
   * 自动发现本机上装了 @types/frida-gum 的项目。IDE 打开时用它挑默认来源：
   * 只有一份就直接用（用户什么都不用填），多份取最近改动的那个。
   */
  fridaTypesDirs(): Promise<FridaTypeDir[]> {
    return invokeCommand<FridaTypeDir[]>("hook_frida_types_dirs", {});
  },
  /**
   * 「保存日志」：把**用户在对话框里亲自选中的本机路径**交给桌面端写盘（真终端页在用），
   * 返回落盘的字节数。返回字节数而不是布尔：界面要说"存了 1.2 MB"，
   * 而不是让用户以为"成功=有内容"。
   */
  exportSave(path: string, content: string): Promise<number> {
    return invokeCommand<number>("hook_export_save", { path, content });
  },
};