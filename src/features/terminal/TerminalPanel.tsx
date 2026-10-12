/**
 * 真终端面板（FT1 分页 + FT3 路 B）：**这一块就是 frida 会话的控制台**。
 *
 * 位置口径（第八十四轮，用户明确纠正过的地方）：它在 **frida 页右侧那一整块屏**上，
 * 不是另开的一个子标签——左边 A 区设置、B 区脚本，点脚本 = 在这里起一个官方 frida REPL。
 * 面板自己只管"几块屏、谁在跑、什么时候才许杀进程"，会话状态在 `lib/terminalStore`。
 *
  * 同一份组件在两个地方各挂一次：frida 页右侧那块屏用 `host="local"`（本机 shell + 官方 frida REPL），
 * 设备页的 Shell 子标签用 `host="device"`（每条屏都是 `adb -s <选中的那台> shell -tt`）。
 * **两处各拿一张会话表**，谁也不给谁追加会话；页内切换只改 `hidden` 不卸载——
 * 卸载等于 disposeAll，会把人家正在跑的会话连进程一起收掉。
 *
 * 组件只做四件事：把 tab 画出来、把每块屏挂到它自己的宿主 div 上、
 * 把"量得到的尺寸"转成 fit、把用户意图转成会话表的调用。
 * **一个字节都不在这里处理**（§4.1 那条铁律），会话的生死与状态在 `lib/terminalStore`。
 *
 * 三条不可回退的口径：
 * 1. **新开一个不动之前的**：所有 tab 的终端实例都活着，非活动 tab 只是 `hidden`；
 *    验收动作是"第 2 个跑长任务，切到第 1 个再切回来，输出没丢"。
 * 2. **关运行中的会话要确认**：关 tab 会关掉 PTY，内核随即给前台进程组发 SIGHUP，不可逆。
 * 3. **页面卸载收掉全部会话**（R-08）：不留活进程。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, Download, Plus, X } from "lucide-react";

import { deviceApi } from "@/api/device";
import { envApi } from "@/api/env";
import { hookApi } from "@/api/hook";
import { ptyApi } from "@/api/pty";
import { hasNativeDialog, saveFile } from "@/api/dialog";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import type { FridaSettings } from "@/features/hook/frida/types";
import { deviceShellLaunch } from "@/lib/deviceShell";
import { planFridaSession, type FridaSessionBlock } from "@/lib/fridaSession";
import { collectTranscript, transcriptFileName, transcriptText } from "@/lib/terminalTranscript";
import {
  MAX_TERMINAL_SESSIONS,
  TerminalSessionStore,
  type CreateTabResult,
  type TerminalTab,
} from "@/lib/terminalStore";
import "@xterm/xterm/css/xterm.css";

/**
 * frida 会话起不来的四种原因 → 界面文案。
 *
 * 一个个写成 `t("…")` 字面量，而不是"码 → 键名"的映射表：`scripts/i18n-check.mjs` 只认
 * `t("…")` 这种形状，映射表里的键名打错了闸门不会红（第八十四轮差点埋下这一颗）。
 */
function fridaBlockText(
  block: FridaSessionBlock,
  t: (key: string, params?: Record<string, string | number>) => string,
  pythonPath?: string | null,
): string {
  switch (block) {
    case "no-python":
      return t("hook.terminal.fridaNoPython");
    case "no-frida-tools":
      // 带上解释器路径：她要的就是能直接抄走的那条安装命令
      return t("hook.terminal.fridaNoTools", { python: pythonPath || "python" });
    case "no-device":
      return t("hook.terminal.fridaNoDevice");
    case "no-script":
      return t("hook.terminal.fridaNoScript");
    case "spawn-needs-package":
      return t("hook.terminal.fridaNoPackage");
  }
}

/**
 * 起会话被拒时的三种原因，各说各的。
 * 特别地：`store-disposed`（这张表被收过）以前会一路混成"先说要跑什么程序"——
 * 那是对着一个填得好好的命令框说谎。
 */
function createBlockText(
  created: Exclude<CreateTabResult, { ok: true }>,
  t: (key: string, params?: Record<string, string | number>) => string,
): string {
  switch (created.reason) {
    case "limit":
      // 用表实际给出的上限，不写死：测试里会注入更小的值
      return t("hook.terminal.limit", { max: created.max });
    case "empty-program":
      return t("hook.terminal.emptyProgram");
    // 表被收过 / 起进程本身失败：都不是"你输入错了"，说成一次操作失败
    case "store-disposed":
    case "start-failed":
      return t("hook.terminal.failedCreate");
  }
}

/** 卡住多久算"该说话了"：真机实测正常路径是 0.2 秒级（PTY 起来 + 首屏字节到屏），2 秒已经很宽 */
const STALL_REPORT_MS = 2000;

/**
 * 两份菜单（用户纠正过的分工）：
 * - 标签上：只有 `rename`——标签只负责"这条会话叫什么"；
 * - 屏里：`copy / paste / copyPath / new / close`——都是对这块终端本身做的事。
 * 键名对应词典里的 `hook.terminal.menu.<键>`，文案不在代码里写。
 */
const TAB_MENU_ITEMS = ["rename"] as const;
const PANE_MENU_ITEMS = ["copy", "paste", "copyPath", "new", "close"] as const;

const DOT_CLASS: Record<TerminalTab["status"], string> = {
  running: "bg-emerald-500",
  starting: "bg-sky-400",
  stopped: "bg-amber-500",
  exited: "bg-muted-foreground/50",
  failed: "bg-destructive",
};

/**
 * 外部（B 区点脚本）发起的一次 frida 会话请求。
 * 设置是**按下那一刻的快照**：会话起来之后她再改目标，不该搅动已经在跑的会话。
 */
export interface FridaLaunchRequest {
  scriptPath: string;
  settings: FridaSettings;
  nonce: number;
}

/**
 * 把"这块屏要跑什么"先写进屏幕里。
 *
 * 理由不是装饰：第八十七轮的现场是"点了脚本，屏上一片黑，什么提示都没有"，
 * 而日志里连一条 `pty_open` 都没有 ⇒ 我们俩只能靠猜。**一块不该说话的屏，至少要把它打算干什么写在自己脸上。**
 * 命令里只有路径与参数，没有脚本正文（§3.7 那条纪律管的是应用日志，这里也不是日志）。
 */
function stampCommand(store: TerminalSessionStore, id: string, program: string, args: string[]) {
  // **这里不许出现转义字面量**（本项目的护栏一正盯着这个）：想要"淡一点"是审美，
  // 想要"屏上必须有一行我们写的话"是诊断能力。前者可以让步，后者不行。
  store.bundle(id)?.term.write(`> ${program} ${args.join(" ")}\r\n`);
}

/** 这块屏属于哪一页：`local` = 「终端」，`device` = 「设备 shell」 */
export type TerminalPanelHost = "local" | "device";

interface TerminalPanelProps {
  fridaLaunch?: FridaLaunchRequest | null;
  host?: TerminalPanelHost;
  /**
   * 这一块屏现在可见吗。自动开第一个会话**只在可见时**发生：
   * 隐藏的那页偷偷起进程，她既看不见也关不掉（上一版"进页面凭空开会话"就是这个形状）。
   */
  visible?: boolean;
  /**
   * 设备那页**要看哪台**：由页面把"她现在选中的那台序列号"交进来。
   * 面板不许自己去设备列表里挑一台——多设备时那会把会话开到她没选的那台上，
   * 而她后面敲的每条命令都打进那台里（`lib/fridaSession` 对 `-U` 是同一个口径）。
   * 没有序列号就直说"没有在线设备"，一块屏都不开。
   */
  deviceSerial?: string | null;
}

export function TerminalPanel({
  fridaLaunch,
  host = "local",
  visible = true,
  deviceSerial = null,
}: TerminalPanelProps) {
  const { t } = useI18n();
  const storeRef = useRef<TerminalSessionStore | null>(null);
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  /** 本机宿主的默认程序：由桌面端回答（$SHELL），界面不猜 */
  const [defaultProgram, setDefaultProgram] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [presetBusy, setPresetBusy] = useState(false);
  const [saveOutcome, setSaveOutcome] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<number | null>(null);
  const hostsRef = useRef(new Map<string, HTMLDivElement>());
  const refCbsRef = useRef(new Map<string, (el: HTMLDivElement | null) => void>());

  /**
   * 会话表**一张表用到底**（每次 render 新建会把所有 xterm 实例连缓冲区一起丢掉，那正是
   * "切个 tab 输出就没了"的成因）；但它同时必须**能被重建**：
   * React StrictMode 在开发模式下会 mount→cleanup→mount，cleanup 里我们按 R-08 把会话全收了，
   * 如果之后还拿那个**已废弃的旧表**去 create()，表现就是"点了没反应"——
   * 而这次"屏上一片黑、什么提示都没有"的现场，与此同形。
   * 只碰 ref 与 setTabs ⇒ 身份稳定，动作回调可以放心依赖它。
   */
  const ensureStore = useCallback(() => {
    const current = storeRef.current;
    if (current && !current.isDisposed) return current;
    // onChange 闭包里引用 next 本身是安全的：它只在表发生变动后才被调用
    const next = new TerminalSessionStore({
      transport: ptyApi,
      // 整个窗口是"七页常驻"的 ⇒ frida 页那块屏与设备页那块屏**同时在 DOM 里**，
      // 两张表的序号又都从 1 起：前缀不分开，`term-1` 就同时属于两块屏，指不到是谁
      idPrefix: host === "device" ? "dev" : "term",
      onChange: () => setTabs(next.tabs()),
    });
    storeRef.current = next;
    return next;
  }, [host]);
  // **这里不再抓一张表当变量**：动作全部在调用时 `ensureStore()` 现取。
  // 渲染期抓的那一张会被 StrictMode 的假卸载 dispose 掉，之后的点击全打在废表上 ——
  // 第八十九轮用户看到的"先说要跑什么程序"就是这么来的（那其实是"表被收了"，
  // 一句与命令框无关的话被摆在了命令框旁边）。

  useEffect(() => {
    // 设备那页的 ➕ 起的是 adb shell，用不到 $SHELL：不该多问一次桌面
    if (host !== "local") return;
    void ptyApi
      .defaultProgram()
      .then((value) => setDefaultProgram((current) => current || value))
      .catch((reason) => setNotice(String((reason as Error)?.message ?? reason)));
  }, [host]);

  // 离开页面 = 收掉全部会话（R-08）。**只在这一条路径上**批量关会话。
  useEffect(
    () => () => {
      // 收摊：一条活进程都不留（R-08）。**不把 ref 置空**：下一次动作会看到 isDisposed 并重建一张表，
      // 而置空会让"用旧表 create"与"用新表渲染"同时存在（StrictMode 的假卸载就是这个形状）
      storeRef.current?.disposeAll();
    },
    [],
  );

  const active = useMemo(() => tabs.find((tab) => tab.id === activeId) ?? null, [tabs, activeId]);

  /**
   * 每个 tab 一个**稳定身份**的 ref 回调：inline 箭头函数每次 render 都是新函数，
   * React 会先以 null 调一次再以元素调一次，于是每次重渲染都要重新判断挂没挂上。
   * 缓存起来就把这条噪音消掉（StrictMode 的 detach/attach 仍然要能扛，见会话表的 opened）。
   */
  const refFor = useCallback(
    (id: string) => {
      let cb = refCbsRef.current.get(id);
      if (!cb) {
        cb = (el: HTMLDivElement | null) => {
          if (el) hostsRef.current.set(id, el);
          else hostsRef.current.delete(id);
          // 挂必须是"当前这张表"的挂：动作与挂载分家时，就会出现"tab 有了、进程没起"
          if (el) ensureStore().mount(id, el);
        };
        refCbsRef.current.set(id, cb);
      }
      return cb;
    },
    [ensureStore],
  );

  const select = useCallback((id: string) => {
    setActiveId(id);
    // 切回来必须重新量一次：隐藏时量到 0，fit 什么都不改（R-03），
    // "刚变可见"这一刻是唯一能把它对齐的机会（ResizeObserver 也会在这里补一次）
    requestAnimationFrame(() => {
      const live = ensureStore();
      live.fit(id);
      live.repaint(id);
      live.focus(id);
    });
  }, [ensureStore]);

  /**
   * 起会话卡在"starting"超过 2 秒就**主动说话**。
   *
   * 时间常量这么定：真机实测正常路径是 0.2 秒级（PTY 起来 + frida banner 到屏），
   * 2 秒已经很宽；而本轮的原则就是**不许再出现"黑屏 + 零提示 + 日志里什么都没有"**——
   * 用户不该对着一块不表态的屏猜，我们也不该靠她截图来定位。
   */
  /**
   * 卡在"starting"超过 2 秒就主动报告。
   *
   * 计时器**按 tab id 存外面**，不进 effect 依赖：本项目的界面上有多个 react-query 在轮询
   * （前台应用约 5 秒一次，设备列表等更频繁），任何重渲染都会让 `tabs`/`t` 换身份；
   * 如果把 setTimeout 放在这种 effect 的清理里，一次重渲染就把 2 秒计时清零 ——
   * 我上一轮就是这么写出一个"卡住也永远不报"的看门狗的（先记着：护栏要能测，不然等于没有）。
   */
  const stallTimersRef = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  useEffect(() => {
    const timers = stallTimersRef.current;
    const starting = tabs.filter((tab) => tab.status === "starting");
    // 已经不在 starting 的：撤掉它的计时器（正常路径 0.2 秒级就会走到 running/失败）
    timers.forEach((timer, id) => {
      if (!starting.some((tab) => tab.id === id)) {
        clearTimeout(timer);
        timers.delete(id);
      }
    });
    for (const tab of starting) {
      if (timers.has(tab.id)) continue;
      timers.set(
        tab.id,
        setTimeout(() => {
          timers.delete(tab.id);
          const now = ensureStore().tabs().find((item) => item.id === tab.id);
          if (now?.status === "starting") {
            setNotice(t("hook.terminal.stalled", { program: now.program }));
          }
        }, STALL_REPORT_MS),
      );
    }
  }, [ensureStore, t, tabs]);

  /**
   * 设备 shell 会话（H-2，FT2）：**直接起一块屏**，不像 frida 预置那样只填命令。
   *
   * 设备来自 `deviceSerial`（设备页选中的那台），这里**不查设备列表**：
   * 列表是页面级的真相，面板只负责把"这一页选中的那台"拼成一条 `adb -s … shell -tt`。
   *
   * 差别的原因是"还缺不缺参数"：`adb -s <serial> shell -tt` 已经是一个完整可用的交互 shell，
   * 而官方 REPL 必须有目标。实测过 `-tt` 下 Ctrl-C 能中断、改尺寸能传到远端
   * （`stty size` 跟着变），所以这块屏不需要任何新的桌面通道——PTY 不关心跑的是谁。
   *
   * `target` 是**调用那一刻**拿到的那张会话表，一路用它、中途不重取：开发模式的假卸载会把
   * 那张表收掉，那时这里必须**什么都不开**（重取会把会话开进一张没人渲染的新表里——
   * 界面看不见、进程却在跑，正是"僵尸"）。返回值告诉调用方"这块屏真起来了"。
   */
  const createDeviceShell = useCallback(async (target?: TerminalSessionStore): Promise<boolean> => {
    setNotice(null);
    try {
      const store = target ?? ensureStore();
      const env = await deviceApi.environment();
      const built = deviceShellLaunch(env.path, deviceSerial);
      if (!built.ok) {
        setNotice(
          built.block === "no-adb" ? t("hook.terminal.deviceNoAdb") : t("hook.terminal.deviceNoSerial"),
        );
        return false;
      }
      if (store.isDisposed) return false;
      const created = await store.create({ program: built.launch.program, args: built.launch.args, kind: "device" });
      if (!created.ok) {
        // 表刚被收掉：这一页已经不属于我们了，别把"起不来"报在她脸上
        if (created.reason === "store-disposed") return false;
        setNotice(
          created.reason === "limit"
            ? t("hook.terminal.limit", { max: created.max })
            : t("hook.terminal.emptyProgram"),
        );
        return false;
      }
      setActiveId(created.id);
      return true;
    } catch (reason) {
      setNotice(String((reason as Error)?.message ?? reason));
      return false;
    }
  }, [deviceSerial, ensureStore, t]);

  /**
   * ➕ / 菜单里的"新建终端"。**没有命令输入框了**：「终端」那页起系统告诉我们的默认程序
   * （`pty_default_program`，也就是 $SHELL，不猜），「设备 shell」那页起 `adb -s … shell -tt`。
   * 想跑别的东西，就在屏里直接敲——那才是终端该有的用法，而不是我们先替用户填一行命令。
   *
   * 分流按**页**，不按"上一次点过哪个入口"：她那两页里 ➕ 的意思各自是固定的。
   */
  /**
   * ➕ / 菜单里的"新建终端"。**没有命令输入框了**：「终端」那页起系统告诉我们的默认程序
   * （`pty_default_program`，也就是 $SHELL，不猜），「设备 shell」那页起 `adb -s … shell -tt`。
   * 想跑别的东西，就在屏里直接敲——那才是终端该有的用法，而不是我们先替用户填一行命令。
   *
   * 分流按**页**，不按"上一次点过哪个入口"：她那两页里 ➕ 的意思各自是固定的。
   */
  /**
   * ➕ / 菜单里的"新建终端"。**没有命令输入框了**：「终端」那页起系统告诉我们的默认程序
   * （`pty_default_program`，也就是 $SHELL，不猜），「设备 shell」那页起 `adb -s … shell -tt`。
   * 想跑别的东西，就在屏里直接敲——那才是终端该有的用法，而不是我们先替用户填一行命令。
   *
   * 分流按**页**，不按"上一次点过哪个入口"：她那两页里 ➕ 的意思各自是固定的。
   */
  /**
   * ➕ / 菜单里的"新建终端"。**没有命令输入框了**：「终端」那页起系统告诉我们的默认程序
   * （`pty_default_program`，也就是 $SHELL，不猜），「设备 shell」那页起 `adb -s … shell -tt`。
   * 想跑别的东西，就在屏里直接敲——那才是终端该有的用法，而不是我们先替用户填一行命令。
   *
   * 分流按**页**，不按"上一次点过哪个入口"：她那两页里 ➕ 的意思各自是固定的。
   */
  const createNew = useCallback(async () => {
    setNotice(null);
    if (host === "device") {
      await createDeviceShell();
      return;
    }
    const created = await ensureStore().create({ program: defaultProgram, args: [] });
    if (!created.ok) {
      setNotice(createBlockText(created, t));
      return;
    }
    setActiveId(created.id);
    ensureStore().focus(created.id);
  }, [createDeviceShell, defaultProgram, ensureStore, host, t]);

  /**
   * 关一个 tab = **先停止再关闭**。
   *
   * 顺序是有意的：先让会话表关掉 PTY（内核随即给前台进程组发 SIGHUP），再从表里摘掉这块屏，
   * 这样不会出现"界面没了、进程还在"的僵尸。原来那套两步确认按用户要求撤掉了——
   * 一个 ✕ 就该把这件事做完。
   */
  const closeNow = useCallback(
    async (id: string) => {
      const live = ensureStore();
      await live.stop(id);
      await live.close(id, { force: true });
      const rest = live.tabs();
      if (!rest.some((tab) => tab.id === activeId)) setActiveId(rest[rest.length - 1]?.id ?? null);
    },
    [activeId, ensureStore],
  );

  /** 菜单动作：复制选区 / 粘贴 / 复制这块屏在跑什么的路径 */
  const menuAction = useCallback(
    async (kind: "copy" | "paste" | "copyPath", id: string, preselected?: string) => {
      const live = ensureStore();
      setMenu(null);
      if (kind === "copy") {
        const text = preselected ?? live.selection(id);
        if (!text) {
          setNotice(t("hook.terminal.noSelection"));
          return;
        }
        try {
          await navigator.clipboard.writeText(text);
          setNotice(t("hook.terminal.copied", { n: text.length }));
        } catch {
          setNotice(t("hook.terminal.clipboardBlocked"));
        }
        return;
      }
      if (kind === "copyPath") {
        const tab = live.tabs().find((item) => item.id === id);
        if (!tab?.program) return;
        try {
          await navigator.clipboard.writeText(tab.program);
          setNotice(t("hook.terminal.copied", { n: tab.program.length }));
        } catch {
          setNotice(t("hook.terminal.clipboardBlocked"));
        }
        return;
      }
      // 粘贴：读不到剪贴板时告诉她用 ⌘V，而不是假装成功
      try {
        const text = await navigator.clipboard.readText();
        if (text) live.paste(id, text);
      } catch {
        setNotice(t("hook.terminal.clipboardBlocked"));
      }
    },
    [ensureStore, t],
  );

  /**
   * 起一个**官方 frida REPL** 会话（FT3 路 B）。这里不自己拼参数：
   * 环境够不够、连哪台、跑哪个脚本，全在 `lib/fridaSession` 一处判（每种形状都有测试钉着）。
   *
   * 每次都重新探环境、不缓存：她很可能就是刚 `pip install frida-tools` 回来点脚本，
   * 缓存会把"已经装好了"说成"没装"——那是我们自己造的假现场。
   */
  const launchFrida = useCallback(
    async (request: FridaLaunchRequest) => {
      setNotice(null);
      setPresetBusy(true);
      try {
        const python = await envApi.python();
        // 探测本身有剪枝：Python 没就绪时不该去问 frida（问了也问不出东西）
        const frida = python.ready ? await envApi.frida() : null;
        const planned = planFridaSession(python, frida, request.settings, request.scriptPath);
        if (!planned.ok) {
          setNotice(fridaBlockText(planned.block, t, python.path));
          return;
        }
        const live = ensureStore();
        const created = await live.create({
          program: planned.plan.program,
          args: planned.plan.args,
          kind: "frida",
        });
        if (!created.ok) {
          setNotice(
            created.reason === "limit"
              ? t("hook.terminal.limit", { max: created.max })
              : t("hook.terminal.emptyProgram"),
          );
          return;
        }
        setActiveId(created.id);
        stampCommand(live, created.id, planned.plan.program, planned.plan.args);
      } catch (reason) {
        setNotice(String((reason as Error)?.message ?? reason));
      } finally {
        setPresetBusy(false);
      }
    },
    [ensureStore, t],
  );

  /**
   * B 区点一次脚本 = nonce 变一次 = 起一块新屏。
   * 只认 nonce，不认"请求对象在不在"：连着点同一个脚本也要能再起一次，
   * 而对象比较会让 React 每次重渲染都误判成新请求（那是真会多开一个进程的 bug）。
   */
  const lastNonceRef = useRef<number | null>(null);
  useEffect(() => {
    if (!fridaLaunch || fridaLaunch.nonce === lastNonceRef.current) return;
    lastNonceRef.current = fridaLaunch.nonce;
    void launchFrida(fridaLaunch);
  }, [fridaLaunch, launchFrida]);

  /**
   * 「设备 shell」那一页**进去就有一块能敲的屏**（她的用法：点它是开一整页，不是往终端那页追加会话）。
   *
   * 三条闸，每条都是一次真 bug 换来的：
   * - `visible`：**这一页真的在她眼前**才自动开（调用方要把"主 tab + 子标签"两件事一起算好交进来，
   *   因为七页是常驻的）。它的上一版挂在"外部点了没点"上，`useState(0)` 的初值被当成"点了一次"，
   *   于是她一进 frida 页就凭空多出一条没打算要的设备会话。
   * - **按会话表记**，不按布尔量记：开发模式(StrictMode)会把面板挂两次，第一次收尾时那张表
   *   已经被 `disposeAll` 收了；拿布尔量记"开过了"，第二次就不开 → 设备页永远空着。
   *   记"我为哪张表开过"，表被收之后 `ensureStore()` 给的是新表，于是仍然只开一块屏。
   * - 已经自动开过一次就**不再自动开**：她把设备会话全关了，切回来也不再冒会话，要几块点 ➕。
   */
  const autoOpenedForRef = useRef<TerminalSessionStore | null>(null);
  useEffect(() => {
    if (host !== "device" || !visible) return;
    const live = ensureStore();
    if (autoOpenedForRef.current === live) return;
    autoOpenedForRef.current = live;
    if (live.count() === 0) void createDeviceShell(live);
  }, [createDeviceShell, ensureStore, host, visible]);

  /**
   * 保存这块屏的正文（C-15 的终端那一半，D090 的口径）：**只有用户点过一次并确认路径才写**。
   *
   * 三条都是照着结构化控制台那条已经落地的口径对齐的：
   * - 搜索框里有字 ⇒ 只存匹配的行（"存过滤后留在屏幕上的那些行"，不是原始字节流）；
   * - 用户取消**不算失败**，也不写任何东西（连"桌面版才有的对话框"弹不出来时都说清）；
   * - 写完报**字节数**（Rust 那边回读出来的），不是"已保存"三个字——那种话证明不了什么。
   */
  const saveActive = useCallback(async () => {
    if (!active || saving) return;
    const bundle = ensureStore().bundle(active.id);
    if (!bundle) return;
    setSaveOutcome(null);
    const snapshot = collectTranscript(bundle.term, query);
    if (snapshot.lines.length === 0) {
      setSaveOutcome({ ok: false, text: query ? t("hook.terminal.saveNoHit", { query }) : t("hook.terminal.saveEmpty") });
      return;
    }
    setSaving(true);
    try {
      const path = await saveFile({
        title: t("hook.terminal.saveDialog"),
        defaultPath: transcriptFileName(active.program),
        filters: [{ name: t("hook.terminal.saveText"), extensions: ["txt"] }],
      });
      if (!path) {
        // 取消：什么都不发生；只有"这环境根本弹不出对话框"才需要说一句
        if (!hasNativeDialog()) setSaveOutcome({ ok: false, text: t("hook.terminal.saveDesktopOnly") });
        return;
      }
      const bytes = await hookApi.exportSave(path, transcriptText(snapshot));
      setSaveOutcome({
        ok: true,
        text: t("hook.terminal.saved", { lines: snapshot.lines.length, total: snapshot.totalLines, bytes }),
      });
    } catch (reason) {
      setSaveOutcome({ ok: false, text: t("hook.terminal.saveFailed", { message: String((reason as Error)?.message ?? reason) }) });
    } finally {
      setSaving(false);
    }
  }, [active, ensureStore, query, saving, t]);

  const search = useCallback(
    (next: boolean) => {
      if (!active || !query) return;
      const bundle = ensureStore().bundle(active.id);
      if (!bundle) return;
      // **只下指令，不自己数命中**：命中数由 addon-search 的结果事件回来（下面的订阅），
      // 自己按 findNext 的布尔值猜一个数，就是把"没有命中"和"命中 1 处"混成一件事
      if (next) bundle.search.findNext(query);
      else bundle.search.findPrevious(query);
    },
    [active, ensureStore, query],
  );

  const activeIdForSearch = active?.id ?? null;
  useEffect(() => {
    if (!activeIdForSearch) {
      setHits(null);
      return undefined;
    }
    const bundle = ensureStore().bundle(activeIdForSearch);
    if (!bundle) return undefined;
    const disposable = bundle.search.onDidChangeResults((event) => setHits(event.resultCount));
    return () => disposable.dispose();
  }, [activeIdForSearch, ensureStore]);

  // 尺寸跟着容器走：每个 tab 各看自己的宿主（隐藏的那个量到 0，不会改动它的尺寸）
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      // 页面整体重新可见（Hook 子标签切走又切回）也走这里：尺寸一从 0 变成有值，
    // 除了量一次尺寸还要补一次重绘，否则隐藏期间到达的字节就一直不进屏
    hostsRef.current.forEach((el, id) => {
      const live = ensureStore();
      if (live.fit(id)) live.repaint(id);
      else if (el.clientWidth > 0) live.repaint(id);
    });
    });
    hostsRef.current.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [ensureStore, tabs.length]);

  const tabCount = tabs.length;
  useEffect(() => {
    if (tabCount === 0) setActiveId(null);
    else if (!tabs.some((tab) => tab.id === activeId)) setActiveId(tabs[tabCount - 1].id);
  }, [tabCount, tabs, activeId]);

  /**
   * 两个位置、两份菜单（用户纠正）：
   * - **标签上**右键 = 只有「重命名」，那是标签自己的事；
   * - **屏里**右键 = 复制 / 粘贴 / 复制当前路径 / 新建终端 / 关闭，那是终端的操作。
   * 选区在按下右键时就抓下来：等到点"复制"时选区可能已经被清了，那时再取会拿到空串。
   */
  const [menu, setMenu] = useState<
    | { kind: "tab"; id: string; x: number; y: number }
    | { kind: "pane"; id: string; x: number; y: number; selected: string }
    | null
  >(null);
  const [editingId, setEditingId] = useState<string | null>(null);

  const menuRef = useRef<HTMLDivElement | null>(null);

  const openTabMenu = useCallback((e: React.MouseEvent, id: string) => {
    e.preventDefault();
    setMenu({ kind: "tab", id, x: e.clientX, y: e.clientY });
  }, []);

  const openPaneMenu = useCallback(
    (e: React.MouseEvent, id: string) => {
      e.preventDefault();
      const live = ensureStore();
      setMenu({ kind: "pane", id, x: e.clientX, y: e.clientY, selected: live.selection(id) });
    },
    [ensureStore],
  );

  const runMenuItem = useCallback(
    async (key: (typeof PANE_MENU_ITEMS)[number] | "rename", id: string) => {
      if (key === "rename") {
        setMenu(null);
        setEditingId(id);
        return;
      }
      if (key === "new") {
        setMenu(null);
        await createNew();
        return;
      }
      if (key === "close") {
        setMenu(null);
        await closeNow(id);
        return;
      }
      const selected = menu?.kind === "pane" ? menu.selected : undefined;
      setMenu(null);
      await menuAction(key, id, selected);
    },
    [closeNow, createNew, menu, menuAction],
  );

  const startRename = useCallback((id: string) => {
    setMenu(null);
    setEditingId(id);
  }, []);

  const commitRename = useCallback(
    (id: string, value: string) => {
      setEditingId(null);
      // 空串会被会话表拒绝（标题清空后没人认得出这块屏连的是什么），这里静默保留原名
      ensureStore().rename(id, value);
    },
    [ensureStore],
  );

  // 菜单开着时，点别处或按 Esc 都要收起来——固定定位的小菜单不自己关闭就是拦住下一笔操作
  useEffect(() => {
    if (!menu) return undefined;
    // 点菜单**自己**不能把它关掉：上一版在 window 上监听 mousedown 直接关，
    // 于是"重命名"根本没被点过就先消失了——用户看到的正是"点了没功能"
    const dismiss = (e: MouseEvent) => {
      if (menuRef.current?.contains(e.target as Node)) return;
      setMenu(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("mousedown", dismiss);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", dismiss);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  /**
   * tab 的悬停提示：状态 + 尺寸 + 渲染器。
   * 这些信息从界面上撤下来了（用户：把"会话在跑"那部分去掉），但排查时还得拿得到。
   */
  const tabHint = (tab: TerminalTab): string => {
    const state = (() => {
      switch (tab.status) {
        case "running":
          return t("hook.terminal.running");
        case "starting":
          return t("hook.terminal.starting");
        case "exited":
          // 有码报码；没码有信号报信号；两个都没有就明说不知道——不许假称"已停止"
          return t("hook.terminal.exited", {
            code: tab.exitCode ?? (tab.exitSignal ? `信号 ${tab.exitSignal}` : "无退出码"),
          });
        case "failed":
          return tab.error ?? t("hook.terminal.failed");
        default:
          return t("hook.terminal.stopped");
      }
    })();
    return `${state} · ${tab.cols}×${tab.rows} · ${tab.renderer} · ${tab.program} ${tab.args.join(" ")}`.trim();
  };

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-xl border border-border/70 bg-card shadow-card p-2.5 text-xs">
      {/* tab 条：常驻（0 个会话时也得有地方点 ➕）。
          默认名字是「会话 N」——序号由会话表给，句子由词典出，五语才不会分叉 */}
      <div className="flex shrink-0 flex-wrap items-center gap-1" role="tablist" data-testid="terminal-tabs">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            className={cn(
              "flex items-center gap-1 rounded-t border border-b-0 border-border/60 px-2 py-1",
              tab.id === activeId ? "bg-background font-medium" : "bg-muted/40 text-muted-foreground",
            )}
            title={tabHint(tab)}
            data-testid={`terminal-chip-${tab.id}`}
            onContextMenu={(e) => openTabMenu(e, tab.id)}
          >
            {editingId === tab.id ? (
              <input
                className="h-5 w-32 rounded border border-input bg-background px-1 font-mono text-10px"
                defaultValue={tab.title ?? t("hook.terminal.session", { n: tab.index })}
                autoFocus
                aria-label={t("hook.terminal.menu.rename")}
                data-testid="terminal-rename-input"
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename(tab.id, (e.target as HTMLInputElement).value);
                  if (e.key === "Escape") setEditingId(null);
                }}
                onBlur={(e) => commitRename(tab.id, e.target.value)}
              />
            ) : (
              <button
                type="button"
                role="tab"
                aria-selected={tab.id === activeId}
                className="flex items-center gap-1"
                onClick={() => select(tab.id)}
                onDoubleClick={() => startRename(tab.id)}
                data-testid={`terminal-tab-${tab.id}`}
              >
                <span
                  className={cn("h-1.5 w-1.5 shrink-0 rounded-full", DOT_CLASS[tab.status])}
                  data-testid={`terminal-dot-${tab.id}`}
                />
                <span className="max-w-[10rem] truncate">
                  {tab.title ?? t("hook.terminal.session", { n: tab.index })}
                </span>
              </button>
            )}
            <button
              type="button"
              className="rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
              onClick={() => void closeNow(tab.id)}
              aria-label={t("hook.terminal.close")}
              data-testid={`terminal-close-${tab.id}`}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        ))}
        {/* ➕ 就是"新建终端"：不再单独摆一个按钮占一行 */}
        <button
          type="button"
          className="rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
          disabled={tabCount >= MAX_TERMINAL_SESSIONS || presetBusy}
          onClick={() => void createNew()}
          title={t("hook.terminal.new")}
          aria-label={t("hook.terminal.new")}
          data-testid="terminal-new"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
      </div>

      {menu && (
        <div
          ref={menuRef}
          className="fixed z-50 min-w-40 rounded-md border border-border/70 bg-card p-1 text-10px shadow-lg"
          style={{ left: menu.x, top: menu.y }}
          data-testid="terminal-tab-menu"
        >
          {(menu.kind === "tab" ? TAB_MENU_ITEMS : PANE_MENU_ITEMS).map((key) => (
            <button
              key={key}
              type="button"
              className={cn(
                "block w-full rounded px-2 py-1 text-left hover:bg-muted",
                key === "close" && "text-destructive",
              )}
              onClick={() => void runMenuItem(key, menu.id)}
              data-testid={`terminal-menu-${key}`}
            >
              {t(`hook.terminal.menu.${key}`)}
            </button>
          ))}
        </div>
      )}

      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <input
          className="h-6 w-40 rounded border border-input bg-transparent px-2 font-mono"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setHits(null);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            e.preventDefault();
            search(!e.shiftKey);
          }}
          placeholder={t("hook.terminal.searchPh")}
          disabled={!active}
          data-testid="terminal-search"
        />
        <Button size="sm" variant="ghost" className="h-6 px-1" disabled={!active || !query} onClick={() => search(false)} aria-label={t("hook.terminal.searchPrev")} data-testid="terminal-search-prev">
          <ChevronUp className="h-3.5 w-3.5" />
        </Button>
        <Button size="sm" variant="ghost" className="h-6 px-1" disabled={!active || !query} onClick={() => search(true)} aria-label={t("hook.terminal.searchNext")} data-testid="terminal-search-next">
          <ChevronDown className="h-3.5 w-3.5" />
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="h-6 px-2 text-10px"
          disabled={!active || saving}
          title={query ? t("hook.terminal.saveFilteredHint", { query }) : t("hook.terminal.saveHint")}
          onClick={() => void saveActive()}
          data-testid="terminal-save"
        >
          <Download className="h-3 w-3" />
          {saving ? t("hook.terminal.saving") : t("hook.terminal.save")}
        </Button>
        {saveOutcome && (
          <span
            className={cn("min-w-0 truncate text-10px", saveOutcome.ok ? "text-emerald-500" : "text-destructive")}
            title={saveOutcome.text}
            data-testid="terminal-save-outcome"
          >
            {saveOutcome.text}
          </span>
        )}
        {hits !== null && (
          <span className="text-10px text-muted-foreground" data-testid="terminal-hits">
            {hits > 0 ? t("hook.terminal.hits", { n: hits }) : t("hook.terminal.hitsNone")}
          </span>
        )}
      </div>


      {notice && (
        <p className="shrink-0 break-all text-10px text-destructive" data-testid="terminal-error">
          {notice}
        </p>
      )}

      {/* 每块屏都有自己的宿主：非活动 tab 只是 hidden，实例与缓冲区一直活着（分页那条验收的根） */}
      <div className="relative min-h-0 flex-1">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            ref={refFor(tab.id)}
            hidden={tab.id !== activeId}
            className="absolute inset-0 overflow-hidden rounded border border-border/60 bg-black/85 p-1"
            onContextMenu={(e) => openPaneMenu(e, tab.id)}
            data-testid={tab.id === activeId ? "terminal-host" : `terminal-host-${tab.id}`}
          />
        ))}
      </div>
    </section>
  );
}
