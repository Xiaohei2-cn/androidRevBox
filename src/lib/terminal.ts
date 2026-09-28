/**
 * xterm.js 适配层（FT0.3）。
 *
 * 这里是"用开源内核"的那一句落地的地方：**本文件不解析任何转义序列**。
 * 我们只做三件事——按顺序喂字节、把界面尺寸回传给 PTY、把键盘字节送出去。
 * 光标、SGR、备用屏、宽字符全部交给 xterm 的缓冲区模型。
 * 以后谁想在这里加一条"顺手处理下某个转义"，先回答 §4.1 那条铁律再动手。
 */
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { WebglAddon } from "@xterm/addon-webgl";

export interface TerminalBundle {
  term: Terminal;
  fit: FitAddon;
  serialize: SerializeAddon;
  search: SearchAddon;
  /** 实际生效的渲染器：`webgl` 起不来就是 `dom`（R-02 的退路，也是 jsdom 走的那条） */
  renderer: "webgl" | "dom";
  dispose: () => void;
}

export interface TerminalOptions {
  cols: number;
  rows: number;
  /** 大 dump 的滚动上限：不设就是无界吃内存（C-09） */
  scrollback?: number;
  theme?: "dark" | "light";
  fontSize?: number;
}

/**
 * 造一个终端实例。
 *
 * WebGL 是加速器不是必需品：拿不到上下文（Web 视图不支持、jsdom、显卡被禁）就必须**安静地**退回
 * 内置 DOM 渲染器，而不是把整个终端会话炸掉——那会让"打不开终端"看起来像我们的 bug。
 */
export function createTerminal(options: TerminalOptions): TerminalBundle {
  const term = new Terminal({
    cols: Math.max(20, options.cols),
    rows: Math.max(4, options.rows),
    scrollback: options.scrollback ?? 20000,
    allowProposedApi: true, // unicode-graphemes / 字素簇需要（C-08）
    fontSize: options.fontSize ?? 12,
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    cursorBlink: true,
    // 粘贴不当成"连按回车执行"（C-07）：默认就尊重程序的 bracketed paste 模式；
    // 显式写出来是因为把它设成 true 会**永久**关掉这个模式，那是把一个功能当开关丢了。
    ignoreBracketedPasteMode: false,
  });

  const fit = new FitAddon();
  term.loadAddon(fit);
  const serialize = new SerializeAddon();
  term.loadAddon(serialize);
  const search = new SearchAddon();
  term.loadAddon(search);
  term.loadAddon(new WebLinksAddon());
  // 中文/emoji 的列宽：默认解析按码点算，中英混排的表格会错位（C-08）
  // 这个 addon 激活时自己把 unicode.activeVersion 切成 "15-graphemes"（v6 的 API 变了，
  // v5 的 setUnicodeAware 已经不存在——照着旧教程写会直接编译不过）
  term.loadAddon(new UnicodeGraphemesAddon());

  let renderer: "webgl" | "dom" = "dom";
  // **先探再用**，不能只包一层 try/catch：WebglAddon 是在终端 open 之后才真正建上下文并
  // 取参数的，`loadAddon` 当场不抛、之后才炸——那一炸发生在我们已经对外说"我用 webgl"之后，
  // 界面上就是"终端一片空白"。jsdom 用真 xterm 测这条时正好把它逼了出来（见 R-02）。
  if (webglProbablyWorks()) {
    try {
      const webgl = new WebglAddon();
      // 上下文丢了（最小化、显卡驱动抽风、进程被回收）也必须能退回去，不能留空白面板
      webgl.onContextLoss(() => {
        try {
          webgl.dispose();
        } catch {
          /* 已经没了 */
        }
      });
      term.loadAddon(webgl);
      renderer = "webgl";
    } catch {
      renderer = "dom";
    }
  }

  return {
    term,
    fit,
    serialize,
    search,
    renderer,
    dispose: () => {
      try {
        term.dispose();
      } catch {
        /* 重复 dispose 不该把调用方（往往是组件卸载）带崩 */
      }
    },
  };
}

/** 量一次尺寸并回传。返回 null = 容器还没尺寸（隐藏 tab 里量到 0，R-03）。 */
export function measureFit(bundle: TerminalBundle): { cols: number; rows: number } | null {
  const proposed = bundle.fit.proposeDimensions();
  if (!proposed || !proposed.cols || !proposed.rows) return null;
  return { cols: proposed.cols, rows: proposed.rows };
}

/**
 * 这个环境能不能真的用 WebGL 渲染器。判据不是"能拿到一个对象"，而是**能调用取参数的方法**：
 * 拿到一个不像 OpenGL 上下文的东西（jsdom 的 2D 桩、被禁用/软件渲染返回的残缺对象）就当它可用，
 * 后果是渲染器在异步初始化里抛异常，界面留下一块空白（R-02）。
 * 探一次用的临时 canvas 立刻丢掉，不进 DOM。
 */
function webglProbablyWorks(): boolean {
  try {
    const canvas = document.createElement("canvas");
    const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
    return !!gl && typeof (gl as WebGLRenderingContext).getParameter === "function";
  } catch {
    return false;
  }
}
