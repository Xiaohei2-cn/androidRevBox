import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  CircleStop,
  Languages,
  Play,
  RefreshCw,
  Send,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { AdbNotReadyState } from "@/components/ui/adb-gate";
import {
  deviceApi,
  type ExternalProc,
  type HostedBinary,
  type HostedProbeResult,
  type HostedRunRecord,
  type HostedStdinMode,
  type ListenPort,
} from "@/api/device";
import { pickFiles } from "@/api/dialog";
import { waitForTask } from "@/lib/waitForTask";
import { useDragDropPath } from "@/hooks/useDragDropPath";
import {
  isTightened,
  planHostedUpload,
  rejectReasonText,
  UPLOAD_FILE_MODE,
} from "@/lib/hostedUpload";
import { loadRootPref, saveRootPref, shouldTryRoot } from "@/lib/hostedRoot";
import { stripAnsi } from "@/lib/ansi";
import { aiApi, type AiConfig } from "@/api/ai";
import { agentApi } from "@/api/agent";
import {
  HELP_CANDIDATES,
  PROBE_TIMEOUT_MS,
  argsProblem,
  classifyProbe,
  drillVerdict,
  describeProbeFacts,
  encodeStamp,
  loadLaunchPrefs,
  looksLikeMissingCommand,
  probeCategoryKey,
  probeHasOutput,
  probePreflight,
  repeatedError,
  probeLooksLikeHelp,
  saveLaunchPrefs,
  splitArgs,
  stampOf,
  compareStamp,
  decodeStamp,
  extractOptionCandidates,
  probeHiddenBytes,
  utf8Bytes,
  MAX_STDIN_BYTES,
  type DrillVerdict,
  type LaunchPrefs,
} from "./hostedLaunch";
import { DeviceBar } from "@/components/ui/device-bar";
import { InfoChip } from "@/components/ui/info-chip";
import { useI18n, type TranslateFn } from "@/i18n";
import { useAppNav } from "@/app/nav";
import { cn } from "@/lib/utils";

/**
 * 二进制托管（「二进制」主 tab 的子标签，第四十七轮从 ADB 页挪过来）：管理 /data/local/tmp 下的 ELF 文件。
 * - 上区：Agent `hosted.list` 列出托管目录里的 ELF（文件头 magic 判定，不依赖设备端
 *   `file` 命令）——绿色 = 有执行权限（双击加入下区托管），红色 = 无执行权限
 *   （「赋予权限」走 Agent `hosted.chmod`，只补执行位且幂等）；
 * - 下区：托管清单——「执行」走 Agent `hosted.start`（参数数组 exec，不进 shell），
 *   回显 pid 并给出稳定句柄；行状态由设备端运行表（`hosted.list` 的 runs，5s 轮询）
 *   校正，Desktop 重启或刷新后不丢；端口 chip 复用 Agent `process.ports`；
 * - 「终止」优先走 Agent `hosted.stop`（发信号前用落盘的 start time 复核身份，
 *   PID 易主时拒止而不是照数字杀，并回收自己启动的子进程拿到真实死因）；
 *   没有句柄时退回按 PID + 进程名的 `process.kill`。
 * - Root 开关：勾选时先 `su -c id` 探测，可用才开——chmod/启动/终止/日志读取整链路
 *   仍走 Legacy `su -c`（Agent 以 shell 身份运行，root 属主进程它碰不到）。
 * 所有 adb 调用后端 -s 绑定设备。
 */

/** 一句人话：pid + 谁收养的 + 是不是 root。ppid=1 意味着它爹已退出（fork 成守护进程的形状） */
function describeProcs(procs: ExternalProc[]): string {
  return procs
    .map((p) =>
      [
        `pid ${p.pid}`,
        p.ppid === 1 ? "父进程已退出" : `父进程 ${p.ppid}`,
        p.uid === 0 ? "root" : `uid ${p.uid}`,
      ].join(" · "),
    )
    .join("；");
}

interface HostedRow {
  name: string;
  /** Agent 运行表里的稳定句柄；有它才能按 handle + start time 停止（AR7.3） */
  handle: string | null;
  pid: number | null;
  /** 设备侧运行表说它在跑（事实态，来自 hosted.list 的 runs，不是本地猜测） */
  running: boolean;
  /**
   * 我们正在发指令（启动/终止在途）。以前只有 running 一个字段，
   * 于是"设备说它在跑"把「终止」按钮自己禁掉了 —— 用户报的正是这个：
   * 发现托管进程在跑，但终止是灰的，永远停不掉。
   */
  busy: boolean;
  error: string | null;
  /** 启动时所用的 root 上下文：kill/后续操作必须同身份 */
  root: boolean;
  /**
   * 这个 pid 有没有设备侧运行表背书（AR7.7）。
   * 后端 `tracked:false` 时界面**不写 pid**：那只是桌面知道的一个数字，
   * 设备上没有凭据，刷新后就没人认得它 —— 摆在那儿冒充"本工具在管"就是假话。
   */
  tracked: boolean;
  ports: ListenPort[];
  /** 端口查询进行中标记 */
  portsLoading: boolean;
  /** 备注（用途说明），localStorage 持久化 */
  note: string;
  /**
   * 启动偏好（参数 / 一次性 stdin / 是否保持输入通道 / 保存时的版本指纹）。
   * 按「设备 + 文件」持久化：这台机器上 frida-server 的端口参数不该跟到另一台去，
   * 而同一个文件换了版本之后，旧参数也不能当作仍然适用。
   */
  launch: LaunchPrefs;
  /**
   * 设备记录里回读到的启动参数（UI-6 第一层）。
   *
   * 界面上"当初用什么参数跑的"必须来自设备，不来自这里的输入框：输入框是**下一次**
   * 要用的参数，记录里那条才是**这一次**跑着的进程真正用的。两者不一样时要说清是哪个。
   */
  deviceArgs?: string[];
  /** stdin 通道现状（来自设备记录；undefined = 老 Agent 没告知，只能显示"未知"） */
  stdinMode?: HostedStdinMode | null;
  /** 行内展开（参数与 stdin 那一块） */
  expanded: boolean;
  /** 探测状态（null = 这一轮没探过） */
  probe: ProbeNode | null;
  /** 持续输入草稿与在途标记 */
  feed: string;
  feeding: boolean;
}

/**
 * 一层探测的状态，可嵌套：`children` 的键是下钻用的参数前缀。
 *
 * 为什么做成树：多级 help 的真实形状就是树——`xxx --help` 里说它有 `-U` 这类入口，
 * `xxx -U --help` 又给出一层。而过程态只想要一根进度条，所以这里不存"第几条"
 * 这种中间量：已试的条数就是 results 的长度。
 */
interface ProbeNode {
  running: boolean;
  /** 这一层的路径：[] = 直接探二进制；["-U"] = 探 `xxx -U --help` */
  path: string[];
  results: ProbeRow[];
  children: Record<string, ProbeNode>;
  /**
   * 顶部一句话：预检没过、或整层候选报的是同一句话时写这里。
   * 同一句重复九遍不叫九条信息，叫一条信息重复了九遍。
   */
  banner?: string;
  /**
   * 只有下钻层才有：与父层比对后的判词。
   * `same` 必须明说——把同一份总帮助摆在 `-D` 底下，用户会以为那就是 `-D` 的说明。
   */
  verdict?: DrillVerdict;
}

/** 一条候选的回执：要么有设备回传的事实，要么有这次调用本身的错误 */
interface ProbeRow {
  candidate: string;
  result?: HostedProbeResult;
  error?: string;
  translating: boolean;
  translated?: string;
  translateNote?: string;
}

/** 常驻快捷备注选项（值为写入备注的文本本身，跨语言固定） */
const NOTE_PRESETS = ["frida server", "ida远程调试server", "dumper"] as const;

/** 待停止的那个表外实例：进程身份 + 它属于哪个托管文件（确认框要显示名字） */
type PendingStop = ExternalProc & { name: string };

/** 备注持久化键：设备 serial + 文件名维度，跨重启/重托管保留 */
const noteKey = (serial: string, name: string) => `adb.binary.note.${serial}.${name}`;
const loadNote = (serial: string | null, name: string) =>
  (serial && localStorage.getItem(noteKey(serial, name))) || "";
const saveNote = (serial: string, name: string, value: string) => {
  if (value) localStorage.setItem(noteKey(serial, name), value);
  else localStorage.removeItem(noteKey(serial, name));
};

/**
 * 新托管行的初始值。三个入口（双击加入 / 设备运行表补进来 / 上传后加入）
 * 都必须走这里：漏一个字段就会在运行时拿到 undefined，而那些字段恰好都是
 * "缺了也不报错、只是界面不说实话"的那一类（stdinMode 缺省应当显示未知，
 * 而不是显示成可输入）。
 */
function newHostedRow(
  name: string,
  serial: string | null,
  over: Partial<HostedRow> = {},
): HostedRow {
  return {
    name,
    handle: null,
    pid: null,
    running: false,
    busy: false,
    error: null,
    root: false,
    tracked: false,
    ports: [],
    portsLoading: false,
    note: loadNote(serial, name),
    launch: loadLaunchPrefs(serial, name),
    stdinMode: undefined,
    expanded: false,
    probe: null,
    feed: "",
    feeding: false,
    ...over,
  };
}

/** 下钻最多几层、每层最多给几个入口（再多就没人会在这一屏里读了） */
const MAX_DRILL_DEPTH = 2;
const MAX_DRILL_CHIPS = 6;

/**
 * 界面语言 → 给接口看的语言名。
 *
 * 起始语言固定填英文：托管二进制的 help 文本几乎都是英文，
 * 让接口自己猜容易把"猜错源语言"当成"翻译坏了"。真要改，改这里而不是改界面文案。
 */
/** 分类的颜色：绿=拿到东西了，黄=要留意，红=没起来。不额外暗示"这就是帮助" */
function categoryTone(result?: HostedProbeResult): string {
  if (!result) return "bg-red-500/10 text-red-500";
  switch (classifyProbe(result)) {
    case "output-exited":
      return "bg-emerald-500/10 text-emerald-500";
    case "unusable":
      return "bg-red-500/10 text-red-500";
    default:
      return "bg-amber-500/10 text-amber-500";
  }
}

const TRANSLATE_LANGS: Record<string, string> = {
  "zh-CN": "简体中文",
  en: "English",
  ru: "русский",
  "pt-BR": "português (Brasil)",
  ja: "日本語",
};
const TRANSLATE_SOURCE = "英文";

export function BinaryHosting({ active = true }: { active?: boolean }) {
  const { t, locale } = useI18n();
  /** 「去设置里配翻译接口」用：跳设置页并聚焦那一项（配置属于设置，不属于探测面板） */
  const { gotoConfig } = useAppNav();
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [hosted, setHosted] = useState<HostedRow[]>([]);
  /**
   * 待停止的外部实例（`{name, pid}`）：点「停止进程」先亮一次确认，
   * 因为杀进程不可逆，而我们停的又是"别人启动的"进程——没有回头路可给。
   */
  const [pendingStop, setPendingStop] = useState<PendingStop | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [root, setRoot] = useState(false);
  const [probing, setProbing] = useState(false);
  /**
   * 进行中的上传：文件名 → 任务 id（空串 = 刚点下去还没提交完）。
   * 与行的 running/busy 分开算（D076：一个布尔不许兼表两义）——
   * 正在上传绝不该把那一行的「终止」禁掉。
   */
  const [uploads, setUploads] = useState<Record<string, string>>({});
  /**
   * 探测的中断标记（按文件名）。
   *
   * 用 ref 而不是 state：循环每轮都要读它，而 state 在闭包里是旧值 ——
   * 那样点「中断」要等下一轮渲染之后才生效，用户看到的就是"点了没反应"。
   */
  const probeAbort = useRef<Record<string, boolean>>({});
  /** 翻译接口的本机配置（key 只在后端，这里只有后 4 位） */
  const [aiConfig, setAiConfig] = useState<AiConfig | null>(null);


  const { data: env } = useQuery({
    queryKey: ["adb", "environment"],
    queryFn: deviceApi.environment,
    staleTime: 10_000,
  });
  const adbReady = !!env?.installed;

  const { data: devices = [] } = useQuery({
    queryKey: ["devices", "binary"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);
  useEffect(() => {
    if (!deviceSerial && online.length > 0) setDeviceSerial(online[0].serial);
  }, [deviceSerial, online]);

  const {
    data: binaries = [],
    isLoading,
    isError,
    error: listError,
    refetch,
    isFetching,
  } = useQuery({
    queryKey: ["device", "binaries", deviceSerial],
    queryFn: () => deviceApi.binaries(deviceSerial!),
    enabled: !!deviceSerial,
    retry: false,
  });

  /**
   * Agent 侧托管运行表（AR7.2）：pid 与状态来自设备端 `pid + start time` 对账，
   * 不是前端本地记忆，所以 Desktop 重启、页面刷新后也能恢复「谁真的在跑」。
   */
  const { data: runs = [], refetch: refetchRuns, status: runsStatus } = useQuery({
    queryKey: ["adb", "hosted-runs", deviceSerial],
    queryFn: () => deviceApi.hostedRuns(deviceSerial!),
    enabled: !!deviceSerial,
    refetchInterval: 5_000,
    retry: false,
  });

  // 设备切换：托管区清空（pid 属于旧设备）；Root 按「该设备的偏好 + 现场探测」决定
  useEffect(() => {
    setHosted([]);
    setNotice(null);
    if (!deviceSerial) {
      setRoot(false);
      return;
    }
    if (!shouldTryRoot(loadRootPref(deviceSerial))) {
      setRoot(false);
      return;
    }
    // 没手动选过 → 默认想开，但必须探一次 su：探不过就保持普通执行**并说明**，
    // 安静地把 root 当已生效比不开更糟（shell 起的 frida-server 注入不了别人）
    let cancelled = false;
    setProbing(true);
    void (async () => {
      try {
        const ok = await deviceApi.binarySuCheck(deviceSerial);
        if (cancelled) return;
        setRoot(ok);
        if (!ok) setNotice(t("adb.binary.suFail"));
      } catch (e) {
        if (cancelled) return;
        setRoot(false);
        setNotice(String((e as Error)?.message ?? e));
      } finally {
        if (!cancelled) setProbing(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [deviceSerial, t]);

  /**
   * 用运行表校正/补齐托管行：设备端说在跑就是在跑，说退了就把状态收回去。
   *
   * `runsStatus !== "success"` 时**什么都不做**：拿不到表是"不知道"，不是"没在跑"，
   * 拿它去清 pid 会把一只真在跑的进程显示成未运行（本项目反复栽过的同一类错）。
   */
  useEffect(() => {
    if (runsStatus !== "success") return;
    setHosted((rows) => {
      const next = rows.map((row) => {
        const mine = runs
          .filter((r: HostedRunRecord) => r.name === row.name)
          .sort((a, b) => b.started_at_unix - a.started_at_unix);
        const live = mine.find((r) => r.state === "running");
        if (live) {
          return {
            ...row,
            handle: live.handle,
            pid: live.pid,
            running: true,
            root: live.root,
            tracked: true,
            // 这两个数只认设备：输入框里的参数是"下一次要用的"，不是"这一次用的"
            deviceArgs: live.args,
            stdinMode: live.stdin_mode,
          };
        }
        if (!live && row.pid !== null && !row.tracked && !row.busy) {
          /*
           * 这个 pid 只是桌面手里的一个数（Legacy 支路当场读到的 $!），而设备表里查不到
           * 对应的活记录 —— 它要么已经退了，要么压根没登记上。继续摆着就成了
           * "界面说在跑、设备说没这回事"。收回来之后，如果它其实还在跑，
           * 「表外同名进程」那条会接手显示，界面上仍然有能用的停止入口。
           */
          return { ...row, pid: null, tracked: false, ports: [] };
        }
        const last = mine[0];
        if (last && last.state === "exited" && row.busy) {
          // 刚点过启动，结果它秒退：收掉在途标记，别一直转圈
          return { ...row, busy: false, running: false, pid: last.pid };
        }
        return row;
      });
      const known = new Set(next.map((row) => row.name));
      for (const run of runs) {
        if (run.state !== "running" || known.has(run.name)) continue;
        next.push(
          newHostedRow(run.name, deviceSerial, {
            // 从设备运行表补进来的行：这条当然是设备认得的
            handle: run.handle,
            pid: run.pid,
            tracked: true,
            running: true,
            root: run.root,
            deviceArgs: run.args,
            stdinMode: run.stdin_mode,
          }),
        );
        known.add(run.name);
      }
      return next;
    });
  }, [runs, runsStatus, deviceSerial]);

  const patchRow = (name: string, p: Partial<HostedRow>) =>
    setHosted((hs) => hs.map((h) => (h.name === name ? { ...h, ...p } : h)));

  /**
   * 改启动偏好并立刻持久化（按设备 + 文件）。
   *
   * 写在这里而不是 setState 的 updater 里：React 严格模式会把 updater 调两次，
   * 那会变成"同一个动作往 localStorage 写两遍"——值一样所以没人会发现，
   * 但副作用该留在事件处理器里这条规矩一旦破掉，后面就没人分得清哪次写入是真的。
   */
  const patchLaunch = (row: HostedRow, p: Partial<LaunchPrefs>) => {
    if (!deviceSerial) return;
    const launch = { ...row.launch, ...p };
    saveLaunchPrefs(deviceSerial, row.name, launch);
    patchRow(row.name, { launch });
  };

  /**
   * 改某一层的探测状态。
   *
   * `path` 是下钻层级：[] = 第一层，["-U"] = `xxx -U --help` 那一层。
   * 整棵树不可变重建：直接 mutate 会让 React 拿到同一个对象而跳过重渲染，
   * 表现就是"进度条不动了"。
   */
  const patchNode = (name: string, path: string[], updater: (node: ProbeNode) => ProbeNode) => {
    const walk = (node: ProbeNode | undefined, depth: number): ProbeNode | undefined => {
      if (!node) return undefined;
      if (depth >= path.length) return updater(node);
      const child = walk(node.children[path[depth]], depth + 1);
      if (!child) return node;
      return { ...node, children: { ...node.children, [path[depth]]: child } };
    };
    setHosted((hs) =>
      hs.map((h) => {
        if (h.name !== name || !h.probe) return h;
        const next = walk(h.probe, 0);
        return next ? { ...h, probe: next } : h;
      }),
    );
  };

  /** 中断标记的键：同一行的不同层要能各自中断 */
  const probeKey = (name: string, path: string[]) => [name, ...path].join("\u0000");

  /**
   * 一层探测：把候选表按顺序各起一次。
   *
   * 三条口径：
   * ① 一次只问一个候选，中断 = 不再发下一条（不需要取消协议）；
   * ② **过程中界面只给进度**（用户要求：不要一路刷中间信息），结束才出结果；
   * ③ 命中即停，但文案只说"打完东西就自己退了"，不宣称这是官方用法；
   *    全落空也只说"我试过这些都不像"。
   */
  const probeLevel = async (row: HostedRow, path: string[]) => {
    if (!deviceSerial) return;
    const name = row.name;
    const prefix = [...splitArgs(row.launch.argsText), ...path];
    const key = probeKey(name, path);
    patchNode(name, path, (node) => ({ ...node, running: true, banner: undefined }));
    probeAbort.current[key] = false;
    for (const candidate of HELP_CANDIDATES) {
      if (probeAbort.current[key]) {
        patchNode(name, path, (node) => ({ ...node, running: false }));
        return;
      }
      try {
        const result = await deviceApi.binaryProbe(
          deviceSerial,
          name,
          [...prefix, candidate],
          PROBE_TIMEOUT_MS,
        );
        patchNode(name, path, (node) => ({
          ...node,
          results: [
            ...node.results.filter((r) => r.candidate !== candidate),
            { candidate, result, translating: false },
          ],
        }));
        if (probeLooksLikeHelp(result)) {
          patchNode(name, path, (node) => ({ ...node, running: false }));
          setNotice(t("adb.binary.probeHit", { candidate }));
          return;
        }
      } catch (e) {
        const message = String((e as Error)?.message ?? e);
        patchNode(name, path, (node) => ({
          ...node,
          results: [
            ...node.results.filter((r) => r.candidate !== candidate),
            { candidate, error: message, translating: false },
          ],
        }));
        if (looksLikeMissingCommand(message)) {
          // 前端比后端新（或反过来）：这条要的是重启/重新构建 App，跑满九条只会误导
          patchNode(name, path, (node) => ({
            ...node,
            running: false,
            banner: t("adb.binary.probeRebuildHint"),
          }));
          return;
        }
      }
    }
    patchNode(name, path, (node) => ({
      ...node,
      running: false,
      banner: (() => {
        const same = repeatedError(node.results.map((r) => r.error));
        return same ?? t("adb.binary.probeNone");
      })(),
    }));
  };

  /** 按路径取某一层的节点（翻译与面板都要用，写两份必然漂移） */
  const nodeAt = (node: ProbeNode | null | undefined, path: string[]): ProbeNode | undefined =>
    path.length === 0 ? (node ?? undefined) : nodeAt(node?.children[path[0]], path.slice(1));

  /** 「接着试完」这一层：预检已经过了，不再重复问一次设备 */
  const runProbeAt = async (row: HostedRow, path: string[]) => {
    await probeLevel(row, path);
  };

  /** 建一层节点（父节点下挂 children[key]） */
  const ensureLevel = (name: string, path: string[]) =>
    setHosted((hs) =>
      hs.map((h) => {
        if (h.name !== name) return h;
        const empty = (at: string[]): ProbeNode => ({
          running: false,
          path: at,
          results: [],
          children: {},
        });
        if (path.length === 0) return { ...h, expanded: true, probe: h.probe ?? empty([]) };
        const probe = h.probe ?? empty([]);
        const children: Record<string, ProbeNode> = { ...probe.children };
        let cursor = children;
        path.forEach((token, index) => {
          const last = index === path.length - 1;
          const existing = cursor[token] ?? empty(path.slice(0, index + 1));
          cursor[token] = existing;
          if (!last) cursor = { ...existing.children };
        });
        return { ...h, expanded: true, probe: { ...probe, children } };
      }),
    );

  /**
   * 点「探测帮助」：先预检，再逐条问设备。
   *
   * 预检存在的理由：探测是写操作，Agent 不在线时后端一条都不起。
   * 那句"需要 Agent 在线"必须在动手之前说，而且要有一个能按的出路，
   * 不能留给用户从九行同样的失败里自己悟。
   */
  const runProbe = async (row: HostedRow) => {
    if (!deviceSerial) return;
    ensureLevel(row.name, []);
    try {
      const verdict = probePreflight(await agentApi.status(deviceSerial));
      if (verdict.kind === "agentOffline") {
        patchNode(row.name, [], (node) => ({
          ...node,
          results: [],
          canReconnect: true,
          banner: t("adb.binary.probeNeedAgent", {
            state: verdict.state,
            detail: verdict.detail ?? "-",
          }),
        }));
        return;
      }
      if (verdict.kind === "methodMissing") {
        patchNode(row.name, [], (node) => ({
          ...node,
          results: [],
          canReconnect: true,
          banner: t("adb.binary.probeOldAgent", { version: verdict.agentVersion ?? "-" }),
        }));
        return;
      }
    } catch {
      // 预检读不到不拦路：照旧问设备，真原因仍会出现在第一条候选里
    }
    await probeLevel(row, []);
  };

  /** 重连 Agent 后接着探：让"会话断了"有一种不用去设备页找按钮的出路 */
  const reconnectAndRetry = async (row: HostedRow) => {
    if (!deviceSerial) return;
    setProbing(true);
    try {
      await agentApi.restart(deviceSerial);
      setNotice(t("adb.binary.probeReconnected"));
      await runProbe({ ...row, probe: null });
    } catch (e) {
      setNotice(`${t("adb.binary.probeReconnectFail")}: ${String((e as Error)?.message ?? e)}`);
    } finally {
      setProbing(false);
    }
  };

  /**
   * 下钻一层：前缀来自**上一层它自己打出来的帮助文本**。
   *
   * 这就是多级 help（`xxx -U --help` 还有下一层）的做法——不给任何程序写模板，
   * 也不扫二进制里的字符串：程序愿意写进帮助里的选项，就是它承认的入口。
   * 每个前缀最多起 9 次进程、每次都有超时与杀进程组兜底，所以再深也不会失控；
   * 界面只允许向下钻 MAX_DRILL_DEPTH 层，防止点出一棵没法看的树。
   */
  const drillInto = async (
    row: HostedRow,
    path: string[],
    token: string,
    parentText: string,
  ) => {
    if (!deviceSerial) return;
    const next = [...path, token];
    if (next.length > MAX_DRILL_DEPTH) return;
    ensureLevel(row.name, next);
    const argv = [...splitArgs(row.launch.argsText), ...next];
    const ask = async (flag: string) => {
      try {
        return await deviceApi.binaryProbe(
          deviceSerial,
          row.name,
          [...argv, flag],
          PROBE_TIMEOUT_MS,
        );
      } catch (e) {
        return { error: String((e as Error)?.message ?? e) } as const;
      }
    };
    const rows: ProbeRow[] = [];
    let best = "";
    const first = await ask("--help");
    if ("error" in first) {
      rows.push({ candidate: "--help", error: first.error, translating: false });
    } else {
      rows.push({ candidate: "--help", result: first, translating: false });
      if (probeHasOutput(first)) best = first.stdout + first.stderr;
    }
    // 长写法没有内容时才试短写法：有的程序只认 `-h`，反过来也一样
    if (!best) {
      const second = await ask("-h");
      if ("error" in second) {
        rows.push({ candidate: "-h", error: second.error, translating: false });
      } else {
        rows.push({ candidate: "-h", result: second, translating: false });
        if (probeHasOutput(second)) best = second.stdout + second.stderr;
      }
    }
    patchNode(row.name, next, (node) => ({
      ...node,
      results: rows,
      verdict: drillVerdict(stripAnsi(best), parentText, best.trim().length > 0),
    }));
  };

  /**
   * 翻某一条候选的输出。
   *
   * 只翻"有内容的那一条流"（用法常在 stdout、报错在 stderr），
   * 且**翻完之后不再显示原文**（用户口径）：一份文本摆两份，读的人反而要自己核对
   * 哪份才是它说的。原文的字节数与退出码仍在事实行里，判据没被藏起来。
   */
  const translateProbe = async (row: HostedRow, path: string[], candidate: string) => {
    // 取"那一层"的节点：下钻之后的候选不在第一层里，写死 path.length===0 会让
    // 第二层的翻译按钮按下去没反应（第一版就是这样）
    const result = nodeAt(row.probe, path)?.results.find((r) => r.candidate === candidate)?.result;
    const text = result ? stripAnsi(result.stdout.trim() ? result.stdout : result.stderr) : "";
    if (!text) return;
    const at = (node: ProbeNode): ProbeNode => ({
      ...node,
      results: node.results.map((r) =>
        r.candidate === candidate ? { ...r, translating: true, translateNote: undefined } : r,
      ),
    });
    patchNode(row.name, path, at);
    try {
      const out = await aiApi.translate(
        text,
        TRANSLATE_LANGS[locale] ?? "简体中文",
        TRANSLATE_SOURCE,
      );
      patchNode(row.name, path, (node) => ({
        ...node,
        results: node.results.map((r) =>
          r.candidate === candidate
            ? {
                ...r,
                translating: false,
                translated: out.text,
                translateNote: out.sourceTruncated
                  ? t("adb.binary.translateTruncated", { chars: String(out.sentChars) })
                  : undefined,
              }
            : r,
        ),
      }));
    } catch (e) {
      // 翻译坏了不改判探测：留着原文并说一句为什么
      patchNode(row.name, path, (node) => ({
        ...node,
        results: node.results.map((r) =>
          r.candidate === candidate
            ? {
                ...r,
                translating: false,
                translateNote: t("adb.binary.translateFail", {
                  detail: String((e as Error)?.message ?? e),
                }),
              }
            : r,
        ),
      }));
    }
  };

  /**
   * 运行中持续输入（UI-6 第四层）。
   *
   * 「发送」自动补一个换行（交互式程序基本都按行读），要原样送就用「原样发送」；
   * 「结束输入」发完这段并关闭写入端（EOF）。
   * 写不进去时设备侧会明确回 reason，界面原样转述——不把它圆成"已发送"。
   */
  const sendFeed = async (row: HostedRow, mode: "line" | "raw" | "close") => {
    if (!deviceSerial || !row.handle || row.feeding) return;
    const body = row.feed;
    if (mode === "line" && !body.endsWith("\n")) {
      // 补换行这件事写在按钮提示里，不偷偷做：有人就是在等一个不带换行的按键
      patchRow(row.name, { feed: body + "\n" });
    }
    const text = mode === "line" && !body.endsWith("\n") ? body + "\n" : body;
    patchRow(row.name, { feeding: true });
    try {
      const wrote = await deviceApi.hostedWrite(
        deviceSerial,
        row.handle,
        text,
        mode === "close",
      );
      patchRow(row.name, {
        feed: mode === "close" ? row.feed : "",
        feeding: false,
        stdinMode: wrote.stdin_mode,
      });
      setNotice(
        mode === "close"
          ? t("adb.binary.feedClosed", { bytes: String(wrote.bytes_written) })
          : t("adb.binary.feedSent", { bytes: String(wrote.bytes_written) }),
      );
    } catch (e) {
      patchRow(row.name, {
        feeding: false,
        error: t("adb.binary.feedFail", { detail: String((e as Error)?.message ?? e) }),
      });
    }
  };

  const addHosted = (b: HostedBinary) => {
    if (!b.hasExec) return; // 红色不可双击托管（先 chmod）
    setHosted((hs) =>
      hs.some((h) => h.name === b.name) ? hs : [...hs, newHostedRow(b.name, deviceSerial)],
    );
  };

  /** 拉取某进程 LISTEN 端口（执行后自动调用；也供手动刷新） */
  const loadPorts = useCallback(
    async (name: string, pid: number, asRoot: boolean) => {
      if (!deviceSerial) return;
      patchRow(name, { portsLoading: true });
      try {
        const ports = await deviceApi.binaryPorts(deviceSerial, pid, asRoot);
        patchRow(name, { ports, portsLoading: false });
      } catch (e) {
        patchRow(name, { ports: [], portsLoading: false });
        setNotice(String((e as Error)?.message ?? e));
      }
    },
    [deviceSerial],
  );

  /** 勾选 Root：先 su -c id 探测；不可用则不开启并提示。手动选择按设备记住 */
  const toggleRoot = async (checked: boolean) => {
    if (!checked) {
      setRoot(false);
      saveRootPref(deviceSerial, false);
      return;
    }
    if (!deviceSerial) return;
    setProbing(true);
    try {
      const ok = await deviceApi.binarySuCheck(deviceSerial);
      if (ok) {
        setRoot(true);
        saveRootPref(deviceSerial, true);
        setNotice(t("adb.binary.suOk"));
      } else {
        setNotice(t("adb.binary.suFail"));
      }
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    } finally {
      setProbing(false);
    }
  };

  /**
   * 上传一批宿主文件到托管目录：`adb push`（TaskService 长任务）→ 等终态 →
   * 回读托管列表确认它真成了"可托管的文件" → 缺执行位就补 0o111（幂等，走 hosted.chmod）。
   *
   * 两点是刻意的：
   * ① 覆盖/在跑只用**当场从设备读回来**的事实判断，不拿本地缓存猜；
   * ② 每个文件单独回结果。一次拖进来三个文件，"哪个成了、哪个被拒、为什么被拒"
   *    必须看得见 —— 静默丢文件是这类入口最坏的写法。
   */
  const uploadPaths = useCallback(
    async (paths: string[]) => {
      if (!deviceSerial) {
        setNotice(t("adb.binary.uploadNoDevice"));
        return;
      }
      const [listed, runs] = await Promise.all([
        deviceApi.binaries(deviceSerial).catch(() => [] as HostedBinary[]),
        deviceApi.hostedRuns(deviceSerial).catch(() => [] as HostedRunRecord[]),
      ]);
      const plan = planHostedUpload(paths, {
        names: new Set(listed.map((b) => b.name)),
        runningNames: new Set(runs.filter((r) => r.state === "running").map((r) => r.name)),
      });
      const lines: string[] = plan.rejected.map((r) => `${r.path}：${rejectReasonText(r.reason)}`);
      for (const item of plan.accepted) {
        setUploads((u) => ({ ...u, [item.name]: "" }));
        try {
          const taskId = await deviceApi.push(deviceSerial, item.local, item.remote);
          setUploads((u) => ({ ...u, [item.name]: taskId }));
          const status = await waitForTask(taskId);
          if (status !== "success") {
            lines.push(t("adb.binary.uploadFailed", { name: item.name, status: String(status) }));
            continue;
          }
          // push 成功 ≠ 能托管：列表只收 ELF，缺执行位也起不动 → 当场回读确认
          const after = await deviceApi
            .binaries(deviceSerial)
            .catch(() => [] as HostedBinary[]);
          const found = after.find((b) => b.name === item.name);
          if (!found) {
            lines.push(t("adb.binary.uploadNotListed", { name: item.name }));
            continue;
          }
          /*
           * 落地权限收紧到 0755：adb push 保留的是设备 umask 后的模式（实测本地 0644 → 设备
           * 0666），补完执行位就是 -rwxrwxrwx —— 托管目录里"谁都能改"的可执行文件，
           * 下次启动跑的是哪个二进制就不由我们说了。以**设备回读的实际模式**为准判断成没成，
           * 不拿"我调用过 chmod"当成功。
           */
          let modeText = found.perms;
          try {
            const done = await deviceApi.fsChmod(deviceSerial, item.remote, UPLOAD_FILE_MODE);
            if (isTightened(done.mode)) {
              modeText = done.mode_text;
            } else {
              lines.push(
                t("adb.binary.uploadLoose", { name: item.name, mode: done.mode_text }),
              );
              modeText = done.mode_text;
            }
          } catch (e) {
            // 收不成也要保证"能跑"：退回旧的纪律——只补执行位，不动其它位
            try {
              await deviceApi.binaryChmod(deviceSerial, item.name, root);
            } catch {
              // 连执行位都没补上：下面照实报，不报成功
            }
            lines.push(
              t("adb.binary.uploadChmodFailed", {
                name: item.name,
                detail: String((e as Error)?.message ?? e),
              }),
            );
          }
          lines.push(
            t(
              item.running
                ? "adb.binary.uploadOkRunning"
                : item.overwrite
                  ? "adb.binary.uploadOkOverwrite"
                  : "adb.binary.uploadOk",
              { name: item.name, mode: modeText },
            ),
          );
        } catch (e) {
          lines.push(`${item.name}：${String((e as Error)?.message ?? e)}`);
        } finally {
          setUploads((u) => {
            const next = { ...u };
            delete next[item.name];
            return next;
          });
        }
      }
      await refetch();
      void refetchRuns();
      setNotice(lines.length > 0 ? lines.join("\n") : t("adb.binary.uploadNothing"));
    },
    [deviceSerial, refetch, refetchRuns, root, t],
  );

  const uploadingNames = Object.keys(uploads);

  const pickAndUpload = useCallback(async () => {
    try {
      const picked = await pickFiles({ title: t("adb.binary.upload") });
      if (picked && picked.length > 0) await uploadPaths(picked);
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  }, [t, uploadPaths]);

  // 访达拖入 → 上传。Tauri 会拦下原生拖放并经 IPC 给出真实绝对路径，
  // HTML5 dataTransfer 在 webview 里只有文件名，拿不到能用的路径（所以必须走这个 hook）。
  const onDropPaths = useCallback(
    (paths: string[]) => {
      void uploadPaths(paths);
    },
    [uploadPaths],
  );
  useDragDropPath({ onPaths: onDropPaths, enabled: active });

  const chmod = async (b: HostedBinary) => {
    if (!deviceSerial) return;
    try {
      await deviceApi.binaryChmod(deviceSerial, b.name, root);
      setNotice(t("adb.binary.chmodOk", { name: b.name }));
      void refetch();
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
    }
  };

  const run = async (row: HostedRow) => {
    // 只挡"在途请求"：绝不能拿 running（设备说它在跑）当守卫 —— 那正是上一轮把
    // 「终止」按钮禁成灰色的同一个混淆。
    if (!deviceSerial || row.busy) return;
    const args = splitArgs(row.launch.argsText);
    // 本地预检只拦"注定被设备拒掉"的形状；参数最终怎么落进 argv 由设备侧说了算
    const problem = argsProblem(args, root);
    if (problem && problem.includes("超过")) {
      patchRow(row.name, { error: problem });
      return;
    }
    const stdinBytes = utf8Bytes(row.launch.stdinText);
    if (stdinBytes > MAX_STDIN_BYTES) {
      patchRow(row.name, {
        error: t("adb.binary.stdinTooLong", {
          bytes: String(stdinBytes),
          max: String(MAX_STDIN_BYTES),
        }),
      });
      return;
    }
    // 启动身份 = 点击瞬间的 Root 开关；写回本行，kill/复查永远同链路
    const asRoot = root;
    patchRow(row.name, { busy: true, error: null, root: asRoot });
    try {
      const result = await deviceApi.binaryRun(
        deviceSerial,
        row.name,
        asRoot,
        args,
        row.launch.stdinText || undefined,
        row.launch.interactive,
      );
      if (!result.started) {
        // 启动前的检查拦下了：它本来就在跑。这里不报红、也不谎称"已启动"，
        // 而是刷新清单让「已在运行」显示出来，按钮随之变成「停止进程」。
        patchRow(row.name, { busy: false, error: null });
        setNotice(result.detail ?? t("adb.binary.alreadyRunning", { pids: String(result.pid) }));
        void refetch();
        return;
      }
      const pid = result.pid;
      /*
       * pid 照写，但它有没有"设备侧凭据"由 `tracked` 决定（AR7.7）：
       * 登记进运行表的显示成普通 pid chip；没登记上的（Agent 未在线、认领被拒）
       * 显示成「已在运行（未登记）」——写「未运行」是假的，按"本工具在管着它"来显示也是假的。
       */
      // 用这组参数成功起过一次 = 认可了它对应的那一版文件：指纹随之确认。
      // 只在 started 的这条分支做（没起成功就不该改用户的确认状态）。
      const bin = binaries.find((b) => b.name === row.name);
      patchRow(row.name, {
        pid,
        tracked: result.tracked,
        running: true,
        busy: false,
        ports: [],
        error: null,
        // 刚用这组参数起过一次 = 用户认可了它，指纹随之确认；
        // 只提示不自动清空，是因为文件被替换过但参数往往仍然能用
        deviceArgs: args,
        ...(bin && row.launch.argsText
          ? { launch: { ...row.launch, stamp: encodeStamp(stampOf(bin)) } }
          : {}),
      });
      if (bin && row.launch.argsText) {
        saveLaunchPrefs(deviceSerial, row.name, {
          ...row.launch,
          stamp: encodeStamp(stampOf(bin)),
        });
      }
      // 成功也可能带话要交代（登记失败的原因）：不能只在失败时才让用户看见
      if (result.detail) setNotice(result.detail);
      // 立刻回查设备表：pid/状态以设备为准，不等 5s 轮询
      void refetchRuns();
      // 端口可能在 listen() 前几十毫秒才绑定：立即拉一次，3s 后再补一次
      void loadPorts(row.name, pid, asRoot);
      window.setTimeout(() => void loadPorts(row.name, pid, asRoot), 3000);
    } catch (e) {
      patchRow(row.name, { busy: false, error: String((e as Error)?.message ?? e) });
    }
  };

  /**
   * 终止托管进程。有句柄（AR7.3）时优先 `hosted.stop`：Agent 会用落盘的 start time
   * 复核身份，PID 易主时拒止而不是照数字杀，也会顺手回收自己启动的子进程拿到死因。
   * 没有句柄（Legacy/root 启动的进程、Agent 未连接）时退回按 PID + 进程名终止。
   */
  /**
   * 停掉一个**不是本工具启动的**同名进程。
   * 走 root=true：Agent 自己以 shell 运行，用户的进程常常是 `su -c` 起的（shell 杀不掉），
   * 而这条通道在设备上先比 `/proc/<pid>/comm` 再发信号 —— 名字对不上就一个信号都不发，
   * 免得拿一个几秒前读到的 pid 去杀掉恰好复用同号的无关进程。
   */
  const stopExternal = async (name: string, pid: number) => {
    if (!deviceSerial) return;
    setPendingStop(null);
    setProbing(true);
    try {
      await deviceApi.binaryKill(deviceSerial, pid, true, name);
      setNotice(t("adb.binary.stoppedExternal", { pid }));
      void refetch();
      void refetchRuns();
    } catch (e) {
      setNotice(`${t("adb.binary.stopFailed")}: ${String((e as Error)?.message ?? e)}`);
    } finally {
      setProbing(false);
    }
  };

  const kill = async (row: HostedRow) => {
    if (!deviceSerial || row.pid === null || row.busy) return;
    patchRow(row.name, { busy: true, error: null });
    try {
      if (row.handle && !row.root) {
        const stopped = await deviceApi.hostedStop(
          deviceSerial,
          row.handle,
          row.pid ?? undefined,
        );
        // 核过身份才算「确认杀的就是它」；未核过时把详情显示出来，不静默当成成功
        if (!stopped.identity_verified && stopped.outcome === "signaled") {
          patchRow(row.name, {
            pid: null,
            running: false,
            busy: false,
            tracked: false,
            ports: [],
            error: t("adb.binary.stopUnverified", {
            name: row.name,
            detail: stopped.record.detail ?? "-",
          }),
          });
          return;
        }
      } else {
        await deviceApi.binaryKill(deviceSerial, row.pid, row.root, row.name);
      }
      /*
       * root 行的终止走提权通道，Agent 那边只剩一条它够不着的记录（进程已被杀掉，
       * 但表里还写着 running）。不顺手放掉的话，下一次轮询会把这一行又点亮成"在跑"，
       * 用户看到的就是"我明明停了它"。Agent 侧对已消失的进程是幂等成功 + 删记录，
       * 所以这里只清账，失败也不改变"进程已经停了"这个事实。
       */
      if (row.handle && row.root && row.pid !== null) {
        try {
          await deviceApi.hostedStop(deviceSerial, row.handle, row.pid);
        } catch {
          /* 记录留着也会在下一次对账时自己变 exited，不因此报失败 */
        }
      }
      patchRow(row.name, {
        handle: null,
        pid: null,
        running: false,
        busy: false,
        ports: [],
        tracked: false,
        error: t("adb.binary.killed", { pid: row.pid }),
      });
      void refetchRuns();
    } catch (e) {
      patchRow(row.name, { busy: false, error: String((e as Error)?.message ?? e) });
    }
  };

  /**
   * 读翻译配置。
   *
   * 界面上**只有开关与"去设置"的入口**，没有输入框：配置 API 接口属于设置页，
   * 挂在托管行里会让"我想知道这程序怎么用"和"我在配 key"两件事挤在同一屏，
   * 也让一个只读探测面板长出保存按钮。
   */
  const loadAi = useCallback(async () => {
    try {
      setAiConfig(await aiApi.getConfig());
    } catch {
      // 读不到配置就当没开：探测本身不该被翻译拖累
      setAiConfig(null);
    }
  }, []);

  /** 输入通道能不能写：只认设备回传的 stdin_mode */
  const feedOpen = (row: HostedRow) => !!row.handle && !row.root && row.stdinMode === "open";

  const feedHint = (row: HostedRow): string => {
    if (row.root) return t("adb.binary.feedRoot");
    switch (row.stdinMode) {
      case "open":
        return t("adb.binary.feedOpen");
      case "once":
        return t("adb.binary.feedOnce");
      case "lost":
        return t("adb.binary.feedLost");
      case "none":
        return t("adb.binary.feedNone");
      default:
        return t("adb.binary.feedUnknown");
    }
  };

  if (!adbReady) {
    return <AdbNotReadyState hint={env?.hint} />;
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 items-center gap-3">
        <p className="text-xs leading-relaxed text-muted-foreground">{t("adb.binary.description")}</p>
      </div>

      <DeviceBar
        online={online}
        selected={deviceSerial}
        onSelect={setDeviceSerial}
        onRefresh={() => void refetch()}
        refreshing={isFetching}
        root={root}
        probing={probing}
        onRootChange={(c) => void toggleRoot(c)}
      />

      {/* 上区：ELF 文件列表 */}
      <section className="flex min-h-0 flex-1 flex-col gap-1" aria-label={t("adb.binary.listTitle")}>
        <div className="flex shrink-0 items-center justify-between">
          <h3 className="text-xs font-semibold">{t("adb.binary.listTitle")}</h3>
          <span className="text-10px text-muted-foreground">{t("adb.binary.listHint")}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden rounded-xl border border-border/70 bg-card shadow-card">
          {isLoading && <p className="p-3 text-xs text-muted-foreground">{t("common.loading")}</p>}
          {isError && (
            <p className="p-3 break-all text-xs leading-relaxed text-destructive">
              {String((listError as Error)?.message ?? listError)}
            </p>
          )}
          {!isLoading && !isError && binaries.length === 0 && (
            <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.empty")}</p>
          )}
          <ul className="divide-y text-xs">
            {binaries.map((b) => (
              <li key={b.name} data-testid={`bin-${b.name}`}>
                <button
                  type="button"
                  className={cn(
                    "flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-accent",
                    !b.hasExec && "cursor-default hover:bg-transparent",
                  )}
                  onDoubleClick={() => addHosted(b)}
                  title={b.hasExec ? t("adb.binary.dblClickAdd") : undefined}
                >
                  <span
                    className={cn(
                      "min-w-0 flex-1 break-all font-mono font-medium",
                      b.hasExec ? "text-emerald-500" : "text-red-500",
                    )}
                  >
                    {b.name}
                  </span>
                  {(b.externalProcs?.length ?? 0) > 0 && (
                    <span
                      className="shrink-0 rounded bg-amber-500/10 px-1.5 py-0.5 text-10px text-amber-500"
                      title={t("adb.binary.externalRunningTip")}
                      data-testid={`external-${b.name}`}
                    >
                      {t("adb.binary.externalRunning", { pids: describeProcs(b.externalProcs) })}
                    </span>
                  )}
                  {/* 权限/大小/操作固定列宽：无按钮行同位占格，右缘垂直对齐 */}
                  <span className="w-[78px] shrink-0 text-right font-mono text-muted-foreground">
                    {b.perms}
                  </span>
                  <span className="w-16 shrink-0 text-right tabular-nums text-muted-foreground">
                    {b.size} B
                  </span>
                  <span className="flex h-6 w-[104px] shrink-0 items-center justify-end">
                    {!b.hasExec && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-6 gap-1 px-2"
                        onClick={(e) => {
                          e.stopPropagation();
                          void chmod(b);
                        }}
                      >
                        <ShieldCheck className="h-3 w-3" />
                        {t("adb.binary.chmod")}
                      </Button>
                    )}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      </section>

      {/* 下区：托管执行 */}
      <section className="flex shrink-0 max-h-[45%] flex-col gap-1" aria-label={t("adb.binary.hostTitle")}>
        <div className="flex shrink-0 items-center justify-between gap-2">
          <h3 className="text-xs font-semibold">{t("adb.binary.hostTitle")}</h3>
          <div className="flex min-w-0 items-center justify-end gap-2">
            {uploadingNames.length > 0 && (
              <span
                className="shrink-0 text-10px text-sky-500"
                data-testid="hosted-uploading"
                title={t("adb.binary.uploadingHint")}
              >
                <RefreshCw className="mr-0.5 inline h-2.5 w-2.5 animate-spin" />
                {t("adb.binary.uploading", { names: uploadingNames.join("、") })}
              </span>
            )}
            <span className="truncate text-10px text-muted-foreground">
              {t("adb.binary.dblClickAdd")}
            </span>
            {/*
              上传入口。没设备就直接禁掉并写明原因：这时候点只会得到一句"要先选一台设备"。
              注意 disabled 的依据是"能不能开始这个动作"，不是设备状态（D076）。
            */}
            <Button
              size="sm"
              variant="outline"
              data-testid="upload-binary"
              className="h-6 shrink-0 gap-1 px-2"
              disabled={!deviceSerial}
              title={!deviceSerial ? t("adb.binary.uploadNoDevice") : t("adb.binary.uploadHint")}
              onClick={() => void pickAndUpload()}
            >
              <Upload className="h-3 w-3" />
              {t("adb.binary.upload")}
            </Button>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded-xl border border-border/70 bg-card shadow-card">
          {hosted.length === 0 ? (
            <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.hostEmpty")}</p>
          ) : (
            <ul className="divide-y text-xs">
              {hosted.map((row) => {
                const bin = binaries.find((b) => b.name === row.name);
                // "已经在跑"有两种：我们起的（有句柄，用既有「终止」）与别人起的（无句柄，
                // 用下面的「停止进程」）。这一行的按钮只能有一个，否则用户不知道该点哪个。
                const outsiders = bin?.externalProcs ?? [];
                const firstOutsider = outsiders[0] ?? null;
                return (
                  <li key={row.name} className="px-3 py-2" data-testid={`hosted-${row.name}`}>
                    <div className="flex items-center gap-3">
                      <InfoChip
                        className="min-w-0 flex-1 text-left"
                        label={`./${row.name}`}
                        title={t("adb.binary.copyCmd", { name: row.name })}
                        testid={`cmd-${row.name}`}
                      />
                      {row.pid !== null && row.tracked ? (
                        <InfoChip
                          label={`pid ${row.pid}${row.root ? " · root" : ""}`}
                          title={t("adb.binary.copyPid")}
                          testid={`pid-${row.name}`}
                        />
                      ) : row.pid !== null && !row.tracked ? (
                        /*
                         * 起来了，但没能登记进设备侧运行表（AR7.7）：写「未运行」是假的，
                         * 写成普通 pid chip 也是假的——那个数只是桌面的记忆，
                         * 软件重启后没人认得它。照实标「已在运行（未登记）」。
                         */
                        <span
                          className="shrink-0 text-amber-500"
                          title={t("adb.binary.runningUntrackedTip")}
                          data-testid={`running-untracked-${row.name}`}
                        >
                          {t("adb.binary.runningUntracked")}
                        </span>
                      ) : firstOutsider ? (
                        /*
                         * 表外有同名进程在跑：这里不能写「未运行」。它只说明"我们
                         * 托管表里没有它的记录"，而设备上的确实在跑 —— 写未运行就是撒谎。
                         */
                        <span
                          className="shrink-0 text-amber-500"
                          data-testid={`running-outside-${row.name}`}
                        >
                          {t("adb.binary.runningOutside")}
                        </span>
                      ) : (
                        <span className="shrink-0 text-muted-foreground">{t("adb.binary.idle")}</span>
                      )}
                      {row.pid !== null ? (
                        <Button
                          size="sm"
                          variant="outline"
                          data-testid={`kill-${row.name}`}
                          className="h-6 shrink-0 gap-1 px-2 text-destructive"
                          // 在跑 ≠ 不能停：只有指令在途（转圈）时才禁，否则"发现托管进程在跑
                          // 却停不掉"就是自相矛盾的灰色按钮（用户实测到的那个 bug）
                          disabled={row.busy}
                          onClick={() => void kill(row)}
                        >
                          {row.busy ? (
                            <RefreshCw className="h-3 w-3 animate-spin" />
                          ) : (
                            <Square className="h-3 w-3" />
                          )}
                          {t("adb.binary.kill")}
                        </Button>
                      ) : firstOutsider !== null ? (
                        /*
                         * 已经有实例在跑 —— 这里不给「执行」：点了只会起一个秒退的进程。
                         * 换成一个「停止进程」按钮（一次确认），停完按钮自己变回「执行」。
                         */
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-6 shrink-0 gap-1 px-2 text-destructive"
                          data-testid={`stop-external-${row.name}`}
                          disabled={probing}
                          onClick={(event) => {
                            event.stopPropagation();
                            if (firstOutsider) {
                              setPendingStop({ ...firstOutsider, name: row.name });
                            }
                          }}
                        >
                          <CircleStop className="h-3 w-3" />
                          {t("adb.binary.stopProcess")}
                        </Button>
                      ) : (
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-6 shrink-0 gap-1 px-2"
                          data-testid={`run-${row.name}`}
                          disabled={row.busy || bin?.hasExec === false}
                          onClick={(event) => {
                            event.stopPropagation();
                            void run(row);
                          }}
                        >
                          {row.busy ? <RefreshCw className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
                          {row.busy ? t("adb.binary.starting") : t("adb.binary.execute")}
                        </Button>
                      )}
                      {row.pid !== null && (
                        <Button
                          size="sm"
                          variant="ghost"
                          className="h-6 shrink-0 px-1.5 text-muted-foreground"
                          disabled={row.portsLoading}
                          title={t("adb.binary.refreshPorts")}
                          onClick={() => row.pid !== null && void loadPorts(row.name, row.pid, row.root)}
                        >
                          <RefreshCw className={cn("h-3 w-3", row.portsLoading && "animate-spin")} />
                        </Button>
                      )}
                      {/* 启动配置（参数 / stdin / 输入通道）：一个开合按钮就够，默认收起 */}
                      <Button
                        size="sm"
                        variant="ghost"
                        className={cn(
                          "h-6 shrink-0 gap-1 px-1.5 text-muted-foreground",
                          row.launch.argsText && "text-foreground",
                        )}
                        data-testid={`launch-toggle-${row.name}`}
                        title={t("adb.binary.argsToggleTip")}
                        onClick={(event) => {
                          event.stopPropagation();
                          patchRow(row.name, { expanded: !row.expanded });
                        }}
                      >
                        <SlidersHorizontal className="h-3 w-3" />
                        {row.launch.argsText ? t("adb.binary.argsFilled") : t("adb.binary.args")}
                      </Button>
                      {/* 探测帮助：只在用户点击时才去设备上起进程，绝不自动跑 */}
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-6 shrink-0 gap-1 px-2"
                        data-testid={`probe-${row.name}`}
                        disabled={!!row.probe?.running}
                        title={t("adb.binary.probeTip")}
                        onClick={(event) => {
                          event.stopPropagation();
                          void runProbe(row);
                          void loadAi();
                        }}
                      >
                        {row.probe?.running ? (
                          <RefreshCw className="h-3 w-3 animate-spin" />
                        ) : (
                          <Sparkles className="h-3 w-3" />
                        )}
                        {t("adb.binary.probe")}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 shrink-0 px-1.5 text-muted-foreground"
                        disabled={row.pid !== null}
                        title={t("adb.binary.removeRow")}
                        onClick={() => setHosted((hs) => hs.filter((h) => h.name !== row.name))}
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </div>
                    {outsiders.length > 0 && (
                      <div
                        className="mt-1.5 flex items-center gap-2 text-10px"
                        data-testid={`external-note-${row.name}`}
                      >
                        <span className="min-w-0 flex-1 text-amber-500">
                          {t("adb.binary.alreadyRunning", { pids: describeProcs(outsiders) })}
                          {" · "}
                          <span className="text-muted-foreground">{t("adb.binary.externalRunningTip")}</span>
                        </span>
                        {pendingStop?.name === row.name && (
                          <>
                            <span className="shrink-0 text-muted-foreground">
                              {t("adb.binary.stopConfirm", { pid: pendingStop.pid })}
                            </span>
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-6 shrink-0 px-2 text-destructive"
                              data-testid={`confirm-stop-${row.name}`}
                              onClick={() => void stopExternal(row.name, pendingStop.pid)}
                            >
                              {t("adb.binary.stopProcess")}
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="h-6 shrink-0 px-2"
                              onClick={() => setPendingStop(null)}
                            >
                              {t("adb.binary.confirmCancel")}
                            </Button>
                          </>
                        )}
                      </div>
                    )}
                    <div className="mt-1.5 flex items-center gap-1.5">
                      <input
                        aria-label={t("adb.binary.noteLabel", { name: row.name })}
                        data-testid={`note-${row.name}`}
                        className="path-selectable h-6 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-xs"
                        placeholder={t("adb.binary.notePlaceholder")}
                        value={row.note}
                        onChange={(e) => {
                          const v = e.target.value;
                          patchRow(row.name, { note: v });
                          if (deviceSerial) saveNote(deviceSerial, row.name, v);
                        }}
                      />
                      <select
                        aria-label={t("adb.binary.noteQuick")}
                        data-testid={`note-quick-${row.name}`}
                        className="h-6 shrink-0 rounded-md border border-input bg-transparent px-1 text-xs text-muted-foreground"
                        value=""
                        onChange={(e) => {
                          if (!e.target.value) return;
                          patchRow(row.name, { note: e.target.value });
                          if (deviceSerial) saveNote(deviceSerial, row.name, e.target.value);
                          e.target.value = "";
                        }}
                      >
                        <option value="">{t("adb.binary.noteQuick")}</option>
                        {NOTE_PRESETS.map((preset) => (
                          <option key={preset} value={preset}>
                            {preset}
                          </option>
                        ))}
                      </select>
                    </div>

                    {/* 设备记录里回读到的参数：这是"这一次在跑的进程当初怎么起的"，
                        与上面输入框里的"下一次要用的"是两件事，必须分开显示 */}
                    {row.running && (row.deviceArgs?.length ?? 0) > 0 && (
                      <p
                        className="path-selectable mt-1.5 break-all font-mono text-10px text-muted-foreground"
                        data-testid={`device-args-${row.name}`}
                        title={t("adb.binary.deviceArgsTip")}
                      >
                        {t("adb.binary.deviceArgs", { args: row.deviceArgs?.join(" ") ?? "" })}
                      </p>
                    )}

                    {row.expanded && (
                      <div
                        className="mt-1.5 flex flex-col gap-1.5 rounded-md border bg-muted/30 p-2"
                        data-testid={`launch-${row.name}`}
                      >
                        <input
                          aria-label={t("adb.binary.argsLabel", { name: row.name })}
                          data-testid={`args-${row.name}`}
                          className="path-selectable h-6 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
                          placeholder={t("adb.binary.argsPlaceholder")}
                          value={row.launch.argsText}
                          onChange={(e) => patchLaunch(row, { argsText: e.target.value })}
                        />
                        {(() => {
                          const problem = argsProblem(splitArgs(row.launch.argsText), root);
                          return problem ? (
                            <p className="text-10px leading-relaxed text-amber-500" data-testid={`args-problem-${row.name}`}>
                              {problem}
                            </p>
                          ) : null;
                        })()}
                        {(() => {
                          const bin = binaries.find((b) => b.name === row.name);
                          if (!row.launch.argsText || !bin) return null;
                          const verdict = compareStamp(
                            decodeStamp(row.launch.stamp),
                            stampOf(bin),
                          );
                          if (verdict === "changed") {
                            return (
                              <div className="flex items-center gap-1.5" data-testid={`stamp-changed-${row.name}`}>
                                <span className="min-w-0 flex-1 text-10px text-amber-500">
                                  {t("adb.binary.stampStale")}
                                </span>
                                <Button
                                  size="sm"
                                  variant="outline"
                                  className="h-5 shrink-0 px-1.5 text-10px"
                                  onClick={() => patchLaunch(row, { stamp: encodeStamp(stampOf(bin)) })}
                                >
                                  {t("adb.binary.stampConfirm")}
                                </Button>
                              </div>
                            );
                          }
                          if (verdict === "unknown") {
                            return (
                              <p className="text-10px text-muted-foreground" data-testid={`stamp-unknown-${row.name}`}>
                                {t("adb.binary.stampUnknown")}
                              </p>
                            );
                          }
                          return null;
                        })()}
                        <textarea
                          aria-label={t("adb.binary.stdinLabel")}
                          data-testid={`stdin-${row.name}`}
                          className="path-selectable h-16 rounded-md border border-input bg-transparent px-2 py-1 font-mono text-xs"
                          placeholder={t("adb.binary.stdinPlaceholder")}
                          value={row.launch.stdinText}
                          onChange={(e) => patchLaunch(row, { stdinText: e.target.value })}
                        />
                        <div className="flex items-center gap-2 text-10px text-muted-foreground">
                          <span className="shrink-0">
                            {utf8Bytes(row.launch.stdinText)} / {MAX_STDIN_BYTES} 字节
                          </span>
                          <label className="flex shrink-0 items-center gap-1">
                            <input
                              type="checkbox"
                              className="h-3 w-3"
                              data-testid={`interactive-${row.name}`}
                              checked={row.launch.interactive}
                              onChange={(e) => patchLaunch(row, { interactive: e.target.checked })}
                            />
                            {t("adb.binary.stdinInteractive")}
                          </label>
                          {root && row.launch.interactive && (
                            <span className="min-w-0 flex-1 text-amber-500" data-testid={`interactive-root-warn-${row.name}`}>
                              {t("adb.binary.stdinInteractiveRoot")}
                            </span>
                          )}
                        </div>
                      </div>
                    )}

                    {/* 运行中的持续输入：只有设备说"通道开着"才给框。
                        给一个看着能输、其实吞字的框，比不给更糟 */}
                    {row.running && row.handle && (
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
                        {feedOpen(row) ? (
                          <>
                            <input
                              aria-label={t("adb.binary.feedLabel", { name: row.name })}
                              data-testid={`feed-${row.name}`}
                              className="h-6 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
                              placeholder={t("adb.binary.feedPlaceholder")}
                              value={row.feed}
                              disabled={row.feeding}
                              onChange={(e) => patchRow(row.name, { feed: e.target.value })}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                  e.preventDefault();
                                  void sendFeed(row, "line");
                                }
                              }}
                            />
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-6 shrink-0 gap-1 px-2"
                              data-testid={`feed-send-${row.name}`}
                              disabled={row.feeding || row.feed === ""}
                              title={t("adb.binary.feedSendTip")}
                              onClick={() => void sendFeed(row, "line")}
                            >
                              <Send className="h-3 w-3" />
                              {t("adb.binary.feedSend")}
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="h-6 shrink-0 px-2 text-10px"
                              data-testid={`feed-raw-${row.name}`}
                              disabled={row.feeding || row.feed === ""}
                              title={t("adb.binary.feedRawTip")}
                              onClick={() => void sendFeed(row, "raw")}
                            >
                              {t("adb.binary.feedRaw")}
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="h-6 shrink-0 px-2 text-10px"
                              data-testid={`feed-close-${row.name}`}
                              disabled={row.feeding}
                              title={t("adb.binary.feedCloseTip")}
                              onClick={() => void sendFeed(row, "close")}
                            >
                              {t("adb.binary.feedClose")}
                            </Button>
                          </>
                        ) : (
                          <span
                            className="min-w-0 flex-1 text-10px text-muted-foreground"
                            data-testid={`feed-hint-${row.name}`}
                          >
                            {feedHint(row)}
                          </span>
                        )}
                      </div>
                    )}

                    {row.probe && (
                      <ProbeLevelView
                        name={row.name}
                        node={row.probe}
                        aiEnabled={Boolean(aiConfig?.enabled)}
                        aiTail={aiConfig?.keyTail ?? ""}
                        targetLang={TRANSLATE_LANGS[locale] ?? "简体中文"}
                        t={t}
                        canReconnect
                        onAbort={(path) => {
                          // 中断：不再发下一条。已试过的结果留着，不改口说"没有"
                          probeKey(row.name, path);
                          probeAbort.current[probeKey(row.name, path)] = true;
                          patchNode(row.name, path, (node) => ({ ...node, running: false }));
                        }}
                        onRetry={async (path) => {
                          const fresh: HostedRow = { ...row, probe: null };
                          await runProbeAt(fresh, path);
                        }}
                        onDrill={async (path, token, parentText) => {
                          await drillInto(row, path, token, parentText);
                        }}
                        onTranslate={(path, candidate) => void translateProbe(row, path, candidate)}
                        onConfig={() => gotoConfig("app.ai.base_url")}
                        onClose={(target) => patchRow(target, { probe: null })}
                        onReconnect={() => void reconnectAndRetry(row)}
                      />
                    )}
                    {row.error && (
                      <p className="mt-1.5 break-all text-11px leading-relaxed text-destructive" data-testid={`err-${row.name}`}>
                        {row.error}
                      </p>
                    )}
                    {row.pid !== null && (
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5" data-testid={`ports-${row.name}`}>
                        {row.ports.length === 0 ? (
                          <span className="text-10px text-muted-foreground">
                            {row.portsLoading ? t("adb.binary.portsLoading") : t("adb.binary.noPorts")}
                          </span>
                        ) : (
                          row.ports.map((p) => {
                            const text = `${p.address}:${p.port}`;
                            return (
                              <InfoChip
                                key={`${p.family}-${text}`}
                                label={text}
                                title={t("adb.binary.copyPort", { family: p.family })}
                                testid={`port-${row.name}-${p.port}`}
                              />
                            );
                          })
                        )}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>

      {notice && (
        <p
          className="shrink-0 break-all whitespace-pre-line rounded-md border bg-muted/40 px-2 py-1 text-xs text-muted-foreground"
          data-testid="binary-notice"
        >
          {notice}
        </p>
      )}
    </div>
  );
}

/** 信息胶囊：纯文本可选中（path-selectable 单击全选/拖选/右键复制），非按钮 */

/**
 * 探测面板（递归渲染每一层）。
 *
 * 两条界面口径是用户明确要的：
 * ① 探测**过程中只给进度**——一路刷九张卡片既读不清也吵；结束才出结果；
 * ② 翻了译文就**不再摆原文**——同一份内容摆两份，读的人还得自己核对哪份算数。
 * 事实行（耗时、码/信号、真实字节数、有没有杀干净）任何情况下都在：那是判据，不是装饰。
 */
function ProbeLevelView(props: {
  name: string;
  node: ProbeNode;
  aiEnabled: boolean;
  aiTail: string;
  targetLang: string;
  t: TranslateFn;
  onAbort: (path: string[]) => void;
  onRetry: (path: string[]) => void;
  onDrill: (path: string[], token: string, parentText: string) => void;
  onTranslate: (path: string[], candidate: string) => void;
  onConfig: () => void;
  onClose: (name: string) => void;
  canReconnect: boolean;
  onReconnect: () => void;
}) {
  const {
    name,
    node,
    aiEnabled,
    targetLang,
    t,
    onAbort,
    onRetry,
    onDrill,
    onTranslate,
    onConfig,
    onClose,
    canReconnect,
    onReconnect,
  } = props;
  const path = node.path;
  const depth = path.length;
  const done = node.results.length;
  const pct = Math.min(100, Math.round((done / HELP_CANDIDATES.length) * 100));
  const next = HELP_CANDIDATES[Math.min(done, HELP_CANDIDATES.length - 1)];
  return (
    <div
      className={depth === 0 ? "flex flex-col gap-1.5" : "ml-3 flex flex-col gap-1.5 border-l pl-2"}
      data-testid={depth === 0 ? `probe-panel-${name}` : `probe-drill-${name}-${path.join("_")}`}
    >
      {depth > 0 && (
        <div className="flex flex-col gap-1">
          <p className="font-mono text-10px text-muted-foreground">
            {t("adb.binary.probeDrillTitle", { args: path.join(" ") })}
          </p>
          {/* 判词只在跑完之后说；same 与 none 必须明说，否则用户会把总帮助当成这个参数的说明 */}
          {!node.running && node.verdict && node.verdict !== "deeper" && (
            <p
              className={cn(
                "text-10px leading-relaxed",
                node.verdict === "same" ? "text-muted-foreground" : "text-amber-500",
              )}
              data-testid={`probe-verdict-${name}-${path.join("_")}`}
            >
              {t(
                node.verdict === "same"
                  ? "adb.binary.probeDrillSame"
                  : "adb.binary.probeDrillNone",
              )}
            </p>
          )}
        </div>
      )}
      {node.running ? (
        <div className="flex items-center gap-2" data-testid={`probe-progress-${name}`}>
          <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-emerald-500 transition-all"
              style={{ width: `${pct}%` }}
            />
          </div>
          <span className="shrink-0 text-10px text-muted-foreground" data-testid={`probe-current-${name}`}>
            {done}/{HELP_CANDIDATES.length} · {next}
          </span>
          <Button
            size="sm"
            variant="outline"
            className="h-6 shrink-0 gap-1 px-2 text-destructive"
            data-testid={`probe-abort-${name}`}
            onClick={() => onAbort(path)}
          >
            <Square className="h-3 w-3" />
            {t("adb.binary.probeAbort")}
          </Button>
        </div>
      ) : (
        <div className="flex flex-col gap-1.5">
          {node.banner && (
            <div
              className="flex flex-wrap items-center gap-1.5 rounded-md border border-amber-500/40 bg-amber-500/10 p-1.5"
              data-testid={`probe-banner-${name}`}
            >
              <span className="min-w-0 flex-1 break-all text-10px leading-relaxed text-amber-500">
                {node.banner}
              </span>
              {canReconnect && depth === 0 && (
                <Button
                  size="sm"
                  variant="outline"
                  className="h-6 shrink-0 gap-1 px-2"
                  data-testid={`probe-reconnect-${name}`}
                  onClick={onReconnect}
                >
                  <RefreshCw className="h-3 w-3" />
                  {t("adb.binary.probeReconnect")}
                </Button>
              )}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-1.5 text-10px">
            <span className="min-w-0 flex-1 text-muted-foreground">
              {t("adb.binary.probeTried", { count: String(done) })}
            </span>
            {done < HELP_CANDIDATES.length && done > 0 && (
              <Button
                size="sm"
                variant="outline"
                className="h-6 shrink-0 px-2"
                data-testid={`probe-continue-${name}`}
                onClick={() => void onRetry(path)}
              >
                {t("adb.binary.probeContinue")}
              </Button>
            )}
            <Button
              size="sm"
              variant="ghost"
              className="h-6 shrink-0 gap-1 px-1.5 text-muted-foreground"
              data-testid={`probe-close-${name}`}
              title={t("adb.binary.probeClose")}
              onClick={() => onClose(name)}
            >
              <X className="h-3 w-3" />
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="h-6 shrink-0 gap-1 px-1.5 text-muted-foreground"
              data-testid={`probe-config-${name}`}
              title={t("adb.binary.translateGoConfig")}
              onClick={onConfig}
            >
              <Settings2 className="h-3 w-3" />
              {aiEnabled ? t("adb.binary.translateOn", { tail: props.aiTail }) : t("adb.binary.translateOff")}
            </Button>
          </div>
          {depth === 0 && (
            <p className="text-10px leading-relaxed text-muted-foreground">
              {t("adb.binary.probeShellOnly")}
            </p>
          )}
          <ul className="flex flex-col gap-1.5">
            {node.results.map((item) => {
              const showTranslation = Boolean(item.translated);
              const streams: Array<["stdout" | "stderr", string]> = showTranslation
                ? []
                : [
                    ["stdout", item.result ? stripAnsi(item.result.stdout) : ""],
                    ["stderr", item.result ? stripAnsi(item.result.stderr) : ""],
                  ];
              const chips =
                item.result && probeLooksLikeHelp(item.result) && depth < MAX_DRILL_DEPTH
                  ? extractOptionCandidates(
                      `${item.result.stdout}\n${item.result.stderr}`,
                      [...HELP_CANDIDATES, ...path],
                    ).slice(0, MAX_DRILL_CHIPS)
                  : [];
              return (
                <li
                  key={item.candidate}
                  className="rounded-md border border-border/60 p-1.5"
                  data-testid={`probe-item-${name}-${path.join("_")}-${item.candidate}`}
                >
                  <div className="flex flex-wrap items-center gap-1.5">
                    <code className="shrink-0 rounded bg-muted px-1 font-mono text-10px">
                      {[...path, item.candidate].join(" ")}
                    </code>
                    <span
                      className={cn(
                        "shrink-0 rounded px-1.5 py-0.5 text-10px",
                        item.result ? categoryTone(item.result) : "bg-red-500/10 text-red-500",
                      )}
                    >
                      {item.result ? t(probeCategoryKey(item.result)) : t("adb.binary.probeCallFail")}
                    </span>
                    <span className="min-w-0 flex-1 break-all text-10px text-muted-foreground">
                      {item.result ? describeProbeFacts(item.result) : item.error}
                    </span>
                    {item.result && probeHasOutput(item.result) && (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-5 shrink-0 gap-1 px-1.5 text-10px"
                        data-testid={`probe-translate-${name}-${item.candidate}`}
                        disabled={item.translating}
                        title={aiEnabled ? t("adb.binary.translateNotice", { url: "" }) : t("adb.binary.translateNeed")}
                        onClick={() => (aiEnabled ? onTranslate(path, item.candidate) : onConfig())}
                      >
                        <Languages className="h-3 w-3" />
                        {item.translating
                          ? t("adb.binary.translating")
                          : aiEnabled
                            ? t("adb.binary.translate", { lang: targetLang })
                            : t("adb.binary.translateGoConfig")}
                      </Button>
                    )}
                  </div>
                  {streams.map(
                    ([stream, text]) =>
                      text && (
                        <div key={stream} className="mt-1">
                          <p className="text-10px text-muted-foreground">{stream}</p>
                          <pre className="path-selectable max-h-40 overflow-auto whitespace-pre-wrap break-all rounded bg-muted/40 p-1 font-mono text-10px">
                            {text}
                          </pre>
                        </div>
                      ),
                  )}
                  {showTranslation && (
                    <div className="mt-1">
                      <p className="text-10px text-muted-foreground">
                        {t("adb.binary.translated", { lang: targetLang })}
                      </p>
                      <pre className="path-selectable max-h-60 overflow-auto whitespace-pre-wrap break-all rounded bg-muted/40 p-1 text-10px">
                        {item.translated}
                      </pre>
                    </div>
                  )}
                  {item.result?.truncated && (
                    <p className="mt-1 text-10px text-amber-500">
                      {t("adb.binary.probeTruncated", { bytes: String(probeHiddenBytes(item.result)) })}
                    </p>
                  )}
                  {item.result?.still_running && (
                    <p className="mt-1 text-10px text-destructive">
                      {t("adb.binary.probeStillRunning", { pid: String(item.result.pid) })}
                    </p>
                  )}
                  {item.translateNote && (
                    <p className="mt-1 break-all text-10px text-muted-foreground">{item.translateNote}</p>
                  )}
                  {chips.length > 0 && (
                    <div className="mt-1 flex flex-wrap items-center gap-1" data-testid={`probe-chips-${name}-${item.candidate}`}>
                      <span className="text-10px text-muted-foreground">
                        {t("adb.binary.probeDrillHint", { candidate: item.candidate })}
                      </span>
                      {chips.map((token) => (
                        <button
                          key={token}
                          type="button"
                          className="shrink-0 rounded border border-input px-1 font-mono text-10px hover:bg-accent disabled:opacity-50"
                          data-testid={`probe-drill-${name}-${item.candidate}-${token}`}
                          disabled={Boolean(node.children[token]?.running)}
                          title={t("adb.binary.probeDrillTip")}
                          onClick={() =>
                            void onDrill(
                              path,
                              token,
                              item.result ? stripAnsi(`${item.result.stdout}\n${item.result.stderr}`) : "",
                            )}
                        >
                          {token}
                        </button>
                      ))}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
          {Object.entries(node.children).map(([token, child]) => (
            <ProbeLevelView
              key={token}
              {...props}
              node={child}
            />
          ))}
        </div>
      )}
    </div>
  );
}
