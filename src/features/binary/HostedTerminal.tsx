import { useCallback, useEffect, useImperativeHandle, useRef, useState, forwardRef } from "react";
import { SquareTerminal, X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";

import { Button } from "@/components/ui/button";
import { PaneHeader } from "@/components/ui/pane-header";
import { ptyApi } from "@/api/pty";
import { useI18n } from "@/i18n";
import { hostedBinaryLaunch, type HostedBinaryBlock } from "@/lib/deviceShell";
import { TerminalSessionStore, type TerminalTab } from "@/lib/terminalStore";
import { cn } from "@/lib/utils";

/** 一次"在这块屏里跑这个文件"的请求；只认 nonce，理由见下面的 effect */
export interface HostedTerminalRequest {
  dir: string;
  name: string;
  args: string[];
  /** 页顶 Root 开关开着时带 true：远端命令换成 `su -c '...'` 起（缺省普通执行） */
  root?: boolean;
  nonce: number;
}

/**
 * 托管二进制的交互终端：二进制托管页自带的一块屏。
 *
 * 为什么单独一块、不复用设备页那个 shell：这一屏跑的是"这一行选中的文件"，
 * 与页面绑定。跳到别的页开屏，用户要点两次、还要在新页找刚才那块；
 * 那一页本身还会默认开一块 shell，混在一起就说不清谁是谁。
 * 这里另建一张会话表（同一个会话库、同一套 xterm），谁开的面板谁收进程：
 * 本页卸载即全部关闭，不会留下一个看不见也关不掉的调试服务端。
 *
 * 起进程用 `adb -s <serial> shell -tt "cd 目录 && './文件' 参数"`
 * （页顶 Root 开关开着时外面再套一层 `su -c`）：
 * 只有 `-tt` 才有 tty（提示符、回显、行编辑、Ctrl-C、resize）。
 * 拼法与引用规则在 lib/deviceShell.ts，那里另有真机实测表。
 */

/**
 * 父组件对这块屏能下的指令。目前只有一件：**停掉进程之后把它那块屏一起收掉**。
 *
 * 为什么这件事得由父组件发起：点「终止」的是「托管执行」那一行，屏却长在本组件里，
 * 两边不接起来的话，进程已经没了、屏还亮着一段跑完的输出，看上去就是"没停干净"。
 * 更要紧的反方向是进程没跟着 shell 退（它挂着那个 tty）：收掉屏 = 把 tty 抽走。
 */
export interface HostedTerminalHandle {
  /**
   * 关掉「这台设备上、跑这个文件」的屏，返回关掉的块数（0 = 本来就没有它的屏）。
   * 按 `serial + name` 双条件匹配：换过设备之后，同名文件那块屏属于上一条连接，
   * 拿它来给当前这一行"陪葬"就是说假话。
   */
  closeFor(serial: string | null, name: string): number;
  /**
   * 收掉这块面板里的**所有**屏，返回收掉的块数。
   *
   * 用在一处：界面切到另一台手机了。那些屏是上一条 `adb` 连接起的会话，
   * 留在当前这台设备的页面里就是拿另一台的输出冒充这一台。
   */
  closeAll(): number;
}

export const HostedTerminal = forwardRef<
  HostedTerminalHandle,
  {
    adbPath: string | null;
    serial: string | null;
    request: HostedTerminalRequest | null;
  }
>(function HostedTerminal({ adbPath, serial, request }, ref) {
  const { t } = useI18n();
  const storeRef = useRef<TerminalSessionStore | null>(null);
  const hostsRef = useRef(new Map<string, HTMLDivElement>());
  const refCbsRef = useRef(new Map<string, (el: HTMLDivElement | null) => void>());
  /** 每块屏是谁开的（设备 + 文件名）：收屏时靠它认"哪一块是这一行的屏" */
  const ownersRef = useRef(new Map<string, { serial: string | null; name: string }>());
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);


  const ensureStore = useCallback(() => {
    const current = storeRef.current;
    if (current && !current.isDisposed) return current;
    const next = new TerminalSessionStore({
      transport: ptyApi,
      // 表要分开，id 前缀也要分开：两张表都从 1 编号，都叫 term-1 就指不清是谁的屏
      idPrefix: "host",
      onChange: () => setTabs(next.tabs()),
    });
    storeRef.current = next;
    return next;
  }, []);

  /** 卸载即收进程：面板都没了还留一个跑着的调试服务端，是最难查的泄漏 */
  useEffect(
    () => () => {
      storeRef.current?.disposeAll();
      storeRef.current = null;
      hostsRef.current.clear();
      refCbsRef.current.clear();
    },
    [],
  );

  const blockText = useCallback(
    (block: HostedBinaryBlock, name: string) => {
      if (block === "no-adb") return t("hook.terminal.deviceNoAdb");
      if (block === "bad-name") return t("hook.terminal.hostedBadName", { name });
      return t("hook.terminal.deviceNoSerial");
    },
    [t],
  );

  /**
   * 请求来了就开屏并**直接把文件跑起来**（命令本身就在参数里，不需要用户再敲一遍）。
   * 只认 nonce：按对象身份判断的话，父组件每次重渲染都会被当成新请求，那是真会多开进程。
   */
  const lastNonceRef = useRef<number | null>(null);
  useEffect(() => {
    if (!request || request.nonce === lastNonceRef.current) return;
    lastNonceRef.current = request.nonce;
    const store = ensureStore();
    setNotice(null);
    void (async () => {
      const built = hostedBinaryLaunch(adbPath, serial, {
        dir: request.dir,
        name: request.name,
        args: request.args,
        root: request.root ?? false,
      });
      if (!built.ok) {
        setNotice(blockText(built.block, request.name));
        return;
      }
      const created = await store.create({
        program: built.launch.program,
        args: built.launch.args,
        kind: "device",
      });
      if (!created.ok) {
        // 表已被收掉（页面正在卸载）：这时候什么都不该报，进程也不该留
        if (created.reason === "store-disposed") return;
        setNotice(
          created.reason === "limit"
            ? t("hook.terminal.limit", { max: created.max })
            : t("hook.terminal.emptyProgram"),
        );
        return;
      }
      // 标签直接写文件名：这里一屏就是一个文件，序号没有信息量
      store.rename(created.id, request.name);
      ownersRef.current.set(created.id, { serial, name: request.name });
      setActiveId(created.id);
      store.focus(created.id);
    })();
  }, [adbPath, blockText, ensureStore, request, serial, t]);


  /** 每块屏的挂载点：回调必须**每个 id 固定一个**，否则每次渲染都 mount(null)→mount(el)，xterm 会被反复搬走 */
  const hostCallback = (id: string) => {
    let cb = refCbsRef.current.get(id);
    if (!cb) {
      cb = (el) => {
        const store = storeRef.current;
        if (el) hostsRef.current.set(id, el);
        else hostsRef.current.delete(id);
        store?.mount(id, el);
      };
      refCbsRef.current.set(id, cb);
    }
    return cb;
  };

  // 尺寸跟容器走；隐藏的那块量到 0，不去改它的窗口大小
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => {
      const store = storeRef.current;
      if (!store) return;
      hostsRef.current.forEach((el, id) => {
        // 从隐藏变可见时除了量尺寸还要补一次重绘：隐藏期间到达的字节不进屏就是"一片黑"
        if (store.fit(id)) store.repaint(id);
        else if (el.clientWidth > 0) store.repaint(id);
      });
    });
    hostsRef.current.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [tabs.length]);

  /**
   * 换屏 / 多开一屏之后补一次 fit：xterm 是按挂载那一刻的容器尺寸算行列的，
   * 不补就会出现"面板很高、屏只占上半截"的难看结果（用户报的等高风险就是它）。
   */
  useEffect(() => {
    const store = storeRef.current;
    if (!store || !activeId) return undefined;
    const raf = requestAnimationFrame(() => {
      if (store.fit(activeId)) store.repaint(activeId);
    });
    return () => cancelAnimationFrame(raf);
  }, [activeId, tabs.length]);

  // 活动 tab 兜底：关掉当前这块后要落到还剩的那块上
  useEffect(() => {
    if (tabs.length === 0) setActiveId(null);
    else if (!tabs.some((tab) => tab.id === activeId)) setActiveId(tabs[tabs.length - 1].id);
  }, [activeId, tabs]);

  const closeOne = (id: string) => {
    void storeRef.current?.close(id, { force: true });
    hostsRef.current.delete(id);
    refCbsRef.current.delete(id);
    ownersRef.current.delete(id);
  };

  /**
   * 父组件（托管执行那一行）停掉进程之后来收屏。
   *
   * 只认「这台设备 + 这个文件」，一次开过两块就收两块；登记表里已经没有对应屏的
   * 残留条目（比如 store 已被收掉）顺手清掉，不算进"关掉了几块"。
   */
  useImperativeHandle(
    ref,
    () => ({
      closeFor(target: string | null, name: string): number {
        const store = storeRef.current;
        if (!store || store.isDisposed) {
          ownersRef.current.clear();
          return 0;
        }
        const alive = new Set(store.tabs().map((tab) => tab.id));
        const matched: string[] = [];
        ownersRef.current.forEach((owner, id) => {
          if (!alive.has(id)) {
            ownersRef.current.delete(id);
            return;
          }
          if (owner.serial === target && owner.name === name) matched.push(id);
        });
        matched.forEach(closeOne);
        return matched.length;
      },
      closeAll(): number {
        const store = storeRef.current;
        if (!store || store.isDisposed) {
          ownersRef.current.clear();
          return 0;
        }
        const ids = store.tabs().map((tab) => tab.id);
        ids.forEach(closeOne);
        return ids.length;
      },
    }),
    // closeOne/refs 全是稳定引用：依赖数组留空，句柄不会被换掉
    [],
  );


  return (
    // 结构刻意与左列那一块**同构**：标题行在框外、框从标题下面开始。
    // 之前标题画在框里，于是右框比左框高出一截、顶边对不齐（用户说的"要和左侧等高"）。
    <div
      className="flex h-full min-h-0 flex-col gap-1"
      aria-label={t("adb.binary.interactive")}
      data-testid="hosted-terminal"
    >
      <PaneHeader
        icon={SquareTerminal}
        title={t("adb.binary.interactive")}
        description={t("adb.binary.terminalHint")}
        actions={
          tabs.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              className="h-6 gap-1 px-1.5 text-11px text-muted-foreground hover:text-destructive"
              data-testid="hosted-terminal-close-all"
              onClick={() => tabs.forEach((tab) => closeOne(tab.id))}
            >
              <X className="h-3 w-3" />
              {t("adb.binary.terminalCloseAll")}
            </Button>
          ) : undefined
        }
      />
      <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border/70 bg-card shadow-card">

        {/* 标签一直给（哪怕只有一块屏）：不说清楚"这块屏正在跑哪个文件"，用户回头就不知道它在干嘛 */}
        {tabs.length > 0 && (
        <div className="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-border/60 px-2 py-1">
          {tabs.map((tab) => (
            <span key={tab.id} className="flex shrink-0 items-center">
              <button
                type="button"
                aria-pressed={tab.id === activeId}
                onClick={() => {
                  setActiveId(tab.id);
                  storeRef.current?.focus(tab.id);
                  storeRef.current?.fit(tab.id);
                  storeRef.current?.repaint(tab.id);
                }}
                className={cn(
                  "max-w-40 truncate rounded-l-md px-2 py-0.5 text-11px",
                  tab.id === activeId
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:bg-accent",
                )}
              >
                {tab.title ?? tab.program}
              </button>
              <button
                type="button"
                aria-label={t("adb.binary.terminalClose", { name: tab.title ?? "" })}
                onClick={() => closeOne(tab.id)}
                className="rounded-r-md px-1 py-0.5 text-muted-foreground hover:bg-accent hover:text-destructive"
              >
                <X className="h-2.5 w-2.5" />
              </button>
            </span>
          ))}
        </div>
        )}

        <div className="relative min-h-0 flex-1">
        {tabs.length === 0 ? (
          <p className="absolute inset-0 flex items-center justify-center px-6 text-center text-11px leading-relaxed text-muted-foreground">
            {t("adb.binary.terminalIdle")}
          </p>
        ) : (
          tabs.map((tab) => (
            <div
              key={tab.id}
              ref={hostCallback(tab.id)}
              hidden={tab.id !== activeId}
              className="absolute inset-0 px-1.5 py-1"
            />
          ))
        )}
        </div>

        {notice && (
          <p className="shrink-0 border-t border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-11px leading-snug text-destructive">
            {notice}
          </p>
        )}
        </div>
      </div>
    );
});
