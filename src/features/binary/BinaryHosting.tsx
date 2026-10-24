import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Select } from "@/components/ui/select";
import { useQuery } from "@tanstack/react-query";
import {
  CircleStop,
  Languages,
  RefreshCw,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  Trash2,
  Upload,
  X,
  Boxes,
  Pencil,
  SquareTerminal,
  PlayCircle,
  Plus,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { PaneHeader } from "@/components/ui/pane-header";
import { AdbNotReadyState } from "@/components/ui/adb-gate";
import {
  deviceApi,
  type ExternalProc,
  type HostedBinary,
  type HostedProbeResult,
  type HostedRunRecord,
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
  encodeStamp,
  loadLaunchPrefs,
  looksLikeMissingCommand,
  probePreflight,
  repeatedError,
  probeLooksLikeHelp,
  saveLaunchPrefs,
  splitArgs,
  stampOf,
  compareStamp,
  decodeStamp,
  judgeProbe,
  probeHiddenBytes,
  type LaunchPrefs,
} from "./hostedLaunch";
import { DeviceBar } from "@/components/ui/device-bar";
import { InfoChip } from "@/components/ui/info-chip";
import { List, ListHeader, ListRow } from "@/components/ui/list";
import { PermissionPill } from "@/components/ui/pill";
import { useI18n, type TranslateFn } from "@/i18n";
import { useAppNav } from "@/app/nav";
import {
  HostedTerminal,
  type HostedTerminalHandle,
  type HostedTerminalRequest,
} from "./HostedTerminal";
import {
  listingIsTrustworthy,
  noteKey,
  orphanRecordNames,
  pickTrackedDevice,
  pruneHostedRecords,
} from "./hostedRecords";

/** 托管目录固定路径（与设备侧 `adb::HOSTED_DIR` 同一个约定）：删记录前回问一次设备要用它 */
const HOSTED_DIR = "/data/local/tmp";
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
  /**
   * 「刚在交互终端里起的那次执行」正在登记进设备侧托管表（在途标记）。
   * 与 busy/running 分家：这里在途的是**补登记**，不是启停指令本身。
   */
  claiming: boolean;
  /** 没登记上的原因（设备侧给的证据）；null = 这一轮没失败过 */
  claimDetail: string | null;
  ports: ListenPort[];
  /** 端口查询进行中标记 */
  portsLoading: boolean;
  /**
   * 这份端口是**为哪个 pid** 查的（null = 没查过）。
   * 端口那一块只在 `portsForPid === pid` 时才出现：没查过就印一句"无 LISTEN 端口"，
   * 那是拿一次没发生过的查询当证据（重新起进程后先冒出来的那句就是这么来的）。
   */
  portsForPid: number | null;
  /** 端口查询失败的原话：查不到就说查不到，不许把空数组说成"没有监听端口" */
  portsError: string | null;

  /**
   * 上一次操作的**结果播报**（"已终止 pid 24208…"这一类）：中性色，单独一格。
   * 以前它跟真错误共用 `error` 那格红色，于是"停止成功"跟"停止失败"长得一样凶，
   * 而且重新起进程之后还挂在行上（用户报的就是这句"已终止 pid 24428"）。
   * 启动那一刻清掉，见 `openInteractiveTerm`。
   * 名字别跟**文件备注**（`writeNote` 顺带同步进来的 `note`）混：那是用户写的字，这是一次操作的回执。
   */
  actionNote: string | null;
  /**
   * 启动偏好（参数 + 保存那一刻的版本指纹）。要输入就在右侧那块屏里输。
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
  /** 行内展开（参数那一块） */
  expanded: boolean;
  /** 探测状态（null = 这一轮没探过） */
  probe: ProbeNode | null;
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
  /** 这一层的路径。探测只做一层，正常恒为 []（保留字段是因为状态机按层级组织） */
  path: string[];
  results: ProbeRow[];
  children: Record<string, ProbeNode>;
  /**
   * 顶部一句话：预检没过、或整层候选报的是同一句话时写这里。
   * 同一句重复九遍不叫九条信息，叫一条信息重复了九遍。
   */
  banner?: string;
  /**
   * 这一层结论的译文。
   *
   * 不按候选存：界面只给一个结论（有/没有），一份结论对应一份译文。
   * 按候选存会让"翻的是哪一条"变成用户要操心的事。
   */
  translating?: boolean;
  translated?: string;
  translateNote?: string;
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
const loadNote = (serial: string | null, name: string) =>
  (serial && localStorage.getItem(noteKey(serial, name))) || "";
const saveNote = (serial: string, name: string, value: string) => {
  if (value) localStorage.setItem(noteKey(serial, name), value);
  else localStorage.removeItem(noteKey(serial, name));
};

/**
 * 新托管行的初始值。三个入口（双击加入 / 设备运行表补进来 / 上传后加入）
 * 都必须走这里：漏一个字段就会在运行时拿到 undefined，而那些字段恰好都是
 * "缺了也不报错、只是界面不说实话"的那一类（端口没查过就得留 null，
 * 而不是让它去显示一句"无 LISTEN 端口"）。
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
    actionNote: null,
    root: false,
    tracked: false,
    claiming: false,
    claimDetail: null,
    ports: [],
    portsLoading: false,
    portsForPid: null,
    portsError: null,
    launch: loadLaunchPrefs(serial, name),
    expanded: false,
    probe: null,
    ...over,
  };
}

/**
 * 界面语言 → 给接口看的语言名。
 *
 * 起始语言固定填英文：托管二进制的 help 文本几乎都是英文，
 * 让接口自己猜容易把"猜错源语言"当成"翻译坏了"。真要改，改这里而不是改界面文案。
 */

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
  /** 本页自己的交互终端请求（一块屏 = 一个文件的一次运行会话） */
  const [termRequest, setTermRequest] = useState<HostedTerminalRequest | null>(null);
  const termNonce = useRef(0);
  /** 上一次选中的设备：只为把"从哪台切到哪台"说出来——换机那一刻这句话最值钱 */
  const lastSerial = useRef<string | null>(null);
  /** 右侧那块屏的句柄：停进程之后要能收掉"它自己那块屏"，见 `closeTerminalFor` */
  const termRef = useRef<HostedTerminalHandle | null>(null);

  /**
   * 收掉这个文件对应的终端屏，返回关掉的块数。
   *
   * 为什么要收：「终止」是按运行表停进程的，屏却是另一条腿（那条 adb shell 会话）。
   * 不一起收的话，进程早没了、屏还亮着一段跑完的输出，看上去就是"没停干净"；
   * 而进程要是挂着这个 tty 没跟着退，收屏等于把 tty 抽走，比留一块僵尸屏有用。
   * 0 块也照常返回：这台设备本来就没为它开过屏（进程是别处起的），提示里不多说一句。
   */
  const closeTerminalFor = (name: string) => termRef.current?.closeFor(deviceSerial, name) ?? 0;

  /** 只在真关掉了屏时才追加那半句：没开过屏不许凭空许诺"屏也关了" */
  const terminalTail = (closed: number) => (closed > 0 ? t("adb.binary.terminalClosedWith") : "");
  const [deviceSerial, setDeviceSerial] = useState<string | null>(null);
  const [hosted, setHosted] = useState<HostedRow[]>([]);

  /**
   * 备注：键是 serial + 文件名，本来就与"是否已托管"无关，所以填在**文件列表**这一行。
   * 写的时候同步一下已托管的同名行，两处显示不能各说各话。
   */
  const [notes, setNotes] = useState<Record<string, string>>({});
  /** 同一时刻只让一行在编辑备注：二十行都挂输入框那是电子表格，不是列表 */
  const [editingNote, setEditingNote] = useState<string | null>(null);
  const noteOf = (name: string) => notes[name] ?? loadNote(deviceSerial, name);
  const writeNote = (name: string, value: string) => {
    setNotes((prev) => ({ ...prev, [name]: value }));
    if (deviceSerial) saveNote(deviceSerial, name, value);
    setHosted((prev) => prev.map((row) => (row.name === name ? { ...row, note: value } : row)));
  };
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

  const { data: devices = [], status: devicesStatus } = useQuery({
    queryKey: ["devices", "binary"],
    queryFn: () => deviceApi.list(true),
    enabled: adbReady,
    refetchInterval: 10_000,
  });
  const online = useMemo(() => devices.filter((d) => d.state === "device"), [devices]);

  /**
   * 选中的设备必须就是 adb 眼前那一台。
   *
   * 用户报的 `内部错误: 读取 /data/local/tmp 失败: adb: device '18271FDF600FL4' not found`
   * 就是这么来的：换了手机，界面还抱着旧序列号发命令，而这条读取失败之后也不会自己回来。
   * 规则要克制，**只有 adb 明确报了别的设备在线时才改选**：
   * 一瞬间枚举到空列表（正在换线 / adb 重启）不该顺手把用户正在用的那块终端屏收掉。
   */
  useEffect(() => {
    if (devicesStatus !== "success") return;
    // 在线的优先；一台都没在线时，也要跟到"adb 报了名字的那台"（新手机还没授权时
    // 它的状态不是 device，但继续抱着已经不存在的旧序列号只会一直报错）
    const candidates = [...online.map((d) => d.serial), ...devices.map((d) => d.serial)];
    const next = pickTrackedDevice(deviceSerial, candidates);
    if (next.serial === deviceSerial) return;
    // 旧连接上的屏不属于现在这台手机：留着就是拿另一台的会话冒充当前设备
    if (next.switchedAway) termRef.current?.closeAll();
    setDeviceSerial(next.serial);
  }, [deviceSerial, devices, devicesStatus, online]);

  /**
   * 那台手机的文件清单**每次都去设备上读**，本机不留副本：
   * `gcTime: 0` 让换设备时旧清单立刻作废（否则切回来先画一屏上一次的列表，
   * 用户读不出它是新是旧，就成了"你缓存手机的清单做什么"）；
   * `refetchInterval` 让"刚才设备不在线、现在插好了"这种失败自己恢复，不用手点刷新。
   */
  const {
    data: listing,
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
    refetchInterval: 20_000,
    gcTime: 0,
  });
  const binaries = listing?.binaries ?? [];

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

  /**
   * 设备切换：托管区清空（pid 属于旧设备）；Root 按「该设备的偏好 + 现场探测」决定。
   * 每台手机的清单、运行表、备注、参数**各管各的**：切过来不许带着上一台的任何状态。
   */
  useEffect(() => {
    const from = lastSerial.current;
    lastSerial.current = deviceSerial;
    setHosted([]);
    setNotice(
      from && deviceSerial && from !== deviceSerial
        ? t("adb.binary.deviceSwitched", { from, to: deviceSerial })
        : null,
    );
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
          }),
        );
        known.add(run.name);
      }
      return next;
    });
  }, [runs, runsStatus, deviceSerial]);

  /**
   * 这台手机上已经不存在的文件，它留下的**本机记录**（备注 + 启动参数）随之删掉。
   *
   * 用户要的正是这条：清单是从这台手机读的，本机只留备注与参数，且**以设备号对应、各台手机互不相干**；
   * 这台手机自己把某个有备注的文件删了，那条记录就不该继续躺着（下次插回别的手机还冒出来更荒谬）。
   *
   * 两道防线，缺一不可：
   * 1. 只有 Agent 那份**完整**清单才有资格怀疑某个文件没了（`listingIsTrustworthy`：
   *    回退腿看不见完整性、被上限截断的、有读不出条目的、空目录的一概不动）；
   * 2. 删之前**逐个回问一次设备**：这条路径还在不在？只有设备明确回答"没有这个文件"（NOT_FOUND）才算数。
   *    扫描与推送之间那几秒钟的重叠是真会发生的——清单里没有、其实刚推上去的文件，
   *    一次 stat 就把它救回来；而查询本身失败（设备掉线、Agent 没连）绝不能被当成"文件没了"，
   *    那是本工具读不到，不是设备上不存在。
   * 一次清单只问一遍（按文件名集合去重），不跟着轮询反复打扰设备。
   */
  const prunedFingerprint = useRef<string | null>(null);
  useEffect(() => {
    const serial = deviceSerial;
    if (!serial || !listing || !listingIsTrustworthy(listing)) return;
    const present = new Set(listing.binaries.map((b) => b.name));
    const fingerprint = `${serial}|${[...present].sort().join(",")}`;
    if (prunedFingerprint.current === fingerprint) return;
    prunedFingerprint.current = fingerprint;
    // 本机有记录、清单里却没有的名字：候选，还不是结论
    const candidates = orphanRecordNames(localStorage, serial, present);
    if (candidates.length === 0) return;
    void (async () => {
      const verdicts = await Promise.all(
        candidates.map(async (name) => {
          try {
            await deviceApi.fileStat(serial, `${HOSTED_DIR}/${name}`);
            // 设备说这个文件还在：一条都不许删（清单快照与 push 之间会撞车）
            return null;
          } catch (e) {
            const code = (e as { code?: string } | null)?.code;
            // 只有设备明确回答"没有这个文件"才算被删；其余错误是本工具读不到，不算证据
            return code === "NOT_FOUND" ? name : null;
          }
        }),
      );
      const gone = verdicts.filter((v): v is string => v !== null);
      if (gone.length === 0) return;
      const removed = pruneHostedRecords(localStorage, serial, new Set(gone));
      if (removed.length > 0) {
        // 删的是用户写过的东西，必须交代删了几条、为什么删
        setNotice(t("adb.binary.recordsPruned", { count: String(removed.length) }));
      }
    })();
  }, [deviceSerial, listing, t]);

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
   * 四条口径：
   * ① 一次只问一个候选，中断 = 不再发下一条（不需要取消协议）；
   * ② **过程中界面只给进度**（用户要求：不要一路刷中间信息），结束才出结果；
   * ③ 命中即停，但文案只说"打完东西就自己退了"，不宣称这是官方用法；
   *    全落空也只说"我试过这些都不像"。
   * ④ **只用探测路径，不拼参数框里的参数**。探的是"这个程序认哪些选项"，
   *    与用户这次打算怎么跑它无关。把参数框垫在前面有两个真后果：
   *    - 结论会错：`stackplz -p 11881 --b -h` 里 `--b` 本身就是坏的，
   *      程序报"未知参数 --b"，我们却会把它记在 `-h` 头上，得出"不支持 -h"；
   *    - 探测是**真的起进程**：带上 `-p <pid>` 这类参数可能真的去挂别人的进程，
   *      一个只想问一句 usage 的操作不该有这种副作用。
   */
  const probeLevel = async (row: HostedRow, path: string[]) => {
    if (!deviceSerial) return;
    const name = row.name;
    const prefix = [...path];
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
          // 命中即停，但不另弹提示：面板上的徽章就是结果，
          // 再报一句"命中 -h"等于把用户不关心的过程又端回来一次
          patchNode(name, path, (node) => ({ ...node, running: false }));
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
    // 徽章自己会写"没有"，这里不再补一遍；只有九条候选报同一句话时才把它顶出来
    // ——那是"哪里坏了"，不是探测结论
    patchNode(name, path, (node) => ({
      ...node,
      running: false,
      banner: repeatedError(node.results.map((r) => r.error)) ?? undefined,
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
   * 翻某一条候选的输出。
   *
   * 只翻"有内容的那一条流"（用法常在 stdout、报错在 stderr），
   * 且**翻完之后不再显示原文**（用户口径）：一份文本摆两份，读的人反而要自己核对
   * 哪份才是它说的。原文的字节数与退出码仍在事实行里，判据没被藏起来。
   */
  /**
   * 翻这一层的结论。
   *
   * 只翻"有内容的那一条流"（帮助常在 stdout、报错在 stderr），
   * 而且翻完只留译文：同一份内容摆两份，读的人还得自己核对哪份算数。
   */
  const translateProbe = async (row: HostedRow, path: string[]) => {
    const verdict = judgeProbe(nodeAt(row.probe, path)?.results ?? []);
    const text = verdict.kind === "help" ? stripAnsi(verdict.text) : "";
    if (!text.trim()) return;
    const mark = (patch: Partial<ProbeNode>) =>
      patchNode(row.name, path, (n) => ({ ...n, ...patch }));
    mark({ translating: true, translateNote: undefined });
    try {
      const out = await aiApi.translate(text, TRANSLATE_LANGS[locale] ?? "简体中文", TRANSLATE_SOURCE);
      mark({
        translating: false,
        translated: out.text,
        translateNote: out.sourceTruncated
          ? t("adb.binary.translateTruncated", { chars: String(out.sentChars) })
          : undefined,
      });
    } catch (e) {
      // 翻译坏了不改判探测：结论与原文照旧，只多说一句为什么没译出来
      mark({
        translating: false,
        translateNote: t("adb.binary.translateFail", {
          detail: String((e as Error)?.message ?? e),
        }),
      });
    }
  };


  const addHosted = (b: HostedBinary) => {
    if (!b.hasExec) return; // 红色不可双击托管（先 chmod）
    setHosted((hs) =>
      hs.some((h) => h.name === b.name) ? hs : [...hs, newHostedRow(b.name, deviceSerial)],
    );
  };

  /**
   * 把「刚在交互终端里起起来的那次执行」登记进设备侧托管运行表。
   *
   * 为什么需要这一步：终端启动走 `adb shell -tt`，那次 exec 全程不经过 Agent，
   * 运行表里没有记录 ⇒ 界面只能说"它在跑，可它不在本工具的托管表里"，
   * 而它明明就是这个页面点起来的（用户报的正是这种"你起的却不认账"）。
   * 桌面在这里只交候选 pid：`knownPids` 之外的同名新进程才是这次的产物；
   * 认不认仍由设备侧按 /proc 实证判（进程名/命令行/属主/启动时刻），说了不算。
   *
   * 没登记上**不算启动失败**：进程确实在跑。行上继续按"表外同名进程"说实话，
   * 只是把猜"可能不是你起的"换成一句有据的原因。
   */
  const claimTerminalLaunch = async (name: string, args: string[], knownPids: number[]) => {
    if (!deviceSerial) return;
    patchRow(name, { claiming: true, claimDetail: null });
    try {
      const result = await deviceApi.binaryClaim(deviceSerial, name, args, knownPids, root);
      if (result.claimed) {
        // 表里有记录了：行的"在跑 / pid / 终止"全部由设备侧回流，这里不自己点亮
        await Promise.all([refetchRuns(), refetch()]);
        patchRow(name, { claiming: false, claimDetail: null });
        return;
      }
      patchRow(name, { claiming: false, claimDetail: result.detail ?? null });
    } catch (e) {
      const message = String((e as Error)?.message ?? e);
      patchRow(name, {
        claiming: false,
        claimDetail: looksLikeMissingCommand(message)
          ? t("adb.binary.claimRebuildHint")
          : message,
      });
    }
  };

  /**
   * 交互跑这个文件（调试服务端那一类：边输出边输入）。就在本页右侧那块屏里开，
   * 不跳去别的页：跳页等于让用户点两次、还要在新页找刚才那块屏。
   * 目录用设备给的路径，参数用该文件存过的启动偏好 —— 与「托管执行」同一份。
   *
   * 开屏之后还要补一次**登记**（`claimTerminalLaunch`）：那次 exec 不经过 Agent，
   * 不补的话运行表里没有它，界面就只能说"在跑但不在托管表里"。
   * `knownPids` 必须在开屏**之前**取：启动前就在跑的同名进程不是这次的产物，
   * 把它们一起交出去等于允许认领别人的进程。
   */
  const openInteractiveTerm = (name: string, argsText: string) => {
    const bin = binaries.find((b) => b.name === name);
    const dir = bin?.path
      ? bin.path.slice(0, bin.path.lastIndexOf("/") + 1) || "/data/local/tmp"
      : "/data/local/tmp";
    const args = splitArgs(argsText);
    const knownPids = (bin?.externalProcs ?? []).map((p) => p.pid);
    // 先把这一行上一次的残留清干净：停完再启动时，行上不该还写着"已终止 pid …"、
    // 也不该带着上一次那个 pid 的端口（那是另一只进程的账）
    patchRow(name, {
      error: null,
      actionNote: null,
      claimDetail: null,
      ports: [],
      portsForPid: null,
      portsError: null,
    });
    termNonce.current += 1;
    setTermRequest({ dir, name, args, root, nonce: termNonce.current });
    void claimTerminalLaunch(name, args, knownPids);
  };



  /**
   * 拉取某进程 LISTEN 端口。**先占住 `portsForPid` 再发请求**：
   * 这样"为哪个 pid 查过"始终有记录，端口那一块也只在查到当前这个 pid 之后才出现。
   * 失败原话留在 `portsError` 里——把查询失败显示成"无 LISTEN 端口"是最容易骗到自己的一种写法。
   */
  const loadPorts = useCallback(
    async (name: string, pid: number, asRoot: boolean) => {
      if (!deviceSerial) return;
      patchRow(name, { portsLoading: true, portsForPid: pid, portsError: null });
      try {
        const ports = await deviceApi.binaryPorts(deviceSerial, pid, asRoot);
        patchRow(name, { ports, portsLoading: false, portsError: null });
      } catch (e) {
        patchRow(name, {
          ports: [],
          portsLoading: false,
          portsError: String((e as Error)?.message ?? e),
        });
      }
    },
    [deviceSerial],
  );

  /**
   * 进程一出现在运行表里就自动查一次端口。
   *
   * 为什么要有这条：老的「执行」那套路是自己起自己查的，启动入口换成交互终端之后没人查了，
   * 于是那一块只能凭一个空数组显示"无 LISTEN 端口"（用户报的第三条就是这个假话）。
   * 判据是"这份端口是不是为当前 pid 查的"，所以一次运行只查一次，pid 换了才重查；
   * `portsLoading` 一并挡着，避免同一次渲染里对同一行发两遍。
   */
  useEffect(() => {
    for (const row of hosted) {
      if (row.pid === null || row.portsLoading || row.portsForPid === row.pid) continue;
      void loadPorts(row.name, row.pid, row.root);
    }
  }, [hosted, loadPorts]);

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
      // 读清单失败就当"不知道有什么"（null），绝不能当成"目录是空的"：
      // 那样上传计划会把同名文件当成新文件推上去，把设备上的东西盖掉
      const [listed, runs] = await Promise.all([
        deviceApi.binaries(deviceSerial).catch(() => null),
        deviceApi.hostedRuns(deviceSerial).catch(() => [] as HostedRunRecord[]),
      ]);
      const plan = planHostedUpload(paths, {
        names: new Set((listed?.binaries ?? []).map((b) => b.name)),
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
          const after = await deviceApi.binaries(deviceSerial).catch(() => null);
          const found = (after?.binaries ?? []).find((b) => b.name === item.name);
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
            // 收权限按这台设备的 root 意图走（与文件页同一个开关）：
            // 覆盖上传时原文件可能是 root 属主的，shell 身份的 chmod 改不动它
            const done = await deviceApi.fsChmod(deviceSerial, item.remote, UPLOAD_FILE_MODE, root);
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
      // 停的是这个文件的进程：为它开过的那块屏一起收掉（没开过就是 0 块，不多说话）
      setNotice(t("adb.binary.stoppedExternal", { pid }) + terminalTail(closeTerminalFor(name)));
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
    patchRow(row.name, { busy: true, error: null, actionNote: null });
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
      // 到这里才是"设备说停了"：上面那条身份没核实的分支已经提前 return，不关屏
      const closed = closeTerminalFor(row.name);
      patchRow(row.name, {
        handle: null,
        pid: null,
        running: false,
        busy: false,
        ports: [],
        portsForPid: null,
        portsError: null,
        tracked: false,
        // 这是**结果播报**不是错误：中性色单独一格，下一次启动时清掉（旧版塞在红色 error 里，
        // 于是"停止成功"跟"停止失败"长得一样，而且重新起进程后那句"已终止"还挂在行上）
        error: null,
        actionNote: t("adb.binary.killed", { pid: row.pid }) + terminalTail(closed),
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

      {/* 左列：文件列表（缩到一半高）+ 托管执行；右列是交互终端 */}
      <div className="flex min-h-0 flex-1 gap-3">
        <div className="flex min-w-0 flex-[0_0_46%] flex-col gap-2">
        {/* 上区：ELF 文件列表 */}
        <section className="flex min-h-0 flex-[0_0_46%] flex-col gap-1" aria-label={t("adb.binary.listTitle")}>
          <PaneHeader
            icon={Boxes}
            title={t("adb.binary.listTitle")}
            description={
              // 轮询/刷新期间说清"正在读的是这台手机"：这份清单本机不留副本，
              // 画出来的每一行都来自设备（用户问过"缓存手机的清单做什么"）
              isFetching && !isLoading ? t("adb.binary.listReading") : t("adb.binary.listHint")
            }
          />
          <div className="pane-scroll min-h-0 flex-1 overflow-y-auto overflow-x-hidden rounded-xl border border-border/70 bg-card shadow-card">
            {isLoading && <p className="p-3 text-xs text-muted-foreground">{t("common.loading")}</p>}
            {isError && (
              <p
                className="p-3 break-all text-xs leading-relaxed text-destructive"
                data-testid="binaries-error"
              >
                {String((listError as Error)?.message ?? listError)}
                {(listError as { code?: string } | null)?.code === "DEVICE_OFFLINE" && (
                  // 设备不在线不是故障：给一句下一步，别顶着一行红字让人以为工具坏了
                  <span className="mt-1 block text-muted-foreground">
                    {t("adb.binary.deviceOfflineTip")}
                  </span>
                )}
              </p>
            )}
            {/* 清单不完整就得说破"这不是全貌"，同时本机记录一条都不许删（见 listingIsTrustworthy） */}
            {listing?.truncated && (
              <p className="p-3 text-10px leading-relaxed text-amber-500" data-testid="binaries-truncated">
                {t("adb.binary.listTruncated")}
              </p>
            )}
            {!!listing && listing.unreadable.length > 0 && (
              <p className="p-3 text-10px leading-relaxed text-amber-500" data-testid="binaries-unreadable">
                {t("adb.binary.listUnreadable", { count: String(listing.unreadable.length) })}
              </p>
            )}
            {!isLoading && !isError && binaries.length === 0 && (
              <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.empty")}</p>
            )}
            <ListHeader testId="bin-list-head" className="pt-1">
              <span className="min-w-0 flex-1">{t("adb.binary.colName")}</span>
              <span className="hidden w-col-perms shrink-0 sm:inline">
                {t("adb.binary.colPerms")}
              </span>
              <span className="w-col-size shrink-0 text-right">{t("adb.binary.colSize")}</span>
              <span className="w-col-actions shrink-0" />
            </ListHeader>
            <List testId="bin-list">
              {binaries.map((b) => (
                <ListRow key={b.name} testId={`bin-${b.name}`} interactive={false}>
                  {/*
                    单行三列（与文件列表同一刻度）：名字 → 权限 → 大小 → 动作。

                    以前这里是「名字 + 备注」纵向两行，再和权限/大小/动作横排。
                    备注一旦有内容，左边那块变高，右边三个固定宽度的格子仍按单行算——
                    权限 `-rwxr-xr-x` 在 74px 里放不下就折成两行，大小也被拆成 `139`/`B`，
                    整行看着像散架。备注是次要信息，挪到名字下面作为第二行、
                    并且**不参与**右侧列的对齐（它跟着名字列走）。
                  */}
                  <div
                    data-testid={`bin-row-${b.name}`}
                    className={cn(
                      "group flex w-full min-w-0 items-center gap-2 text-xs",
                      !b.hasExec && "opacity-70",
                    )}
                    onDoubleClick={() => addHosted(b)}
                    title={b.hasExec ? t("adb.binary.dblClickAdd") : undefined}
                  >
                    {/* 名字列：名字 + 备注（备注只在有内容/编辑时才占第二行） */}
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 items-center gap-2">
                        <span
                          className={cn(
                            "path-selectable min-w-0 truncate font-mono text-sm font-medium leading-tight",
                            b.hasExec ? "text-emerald-600 dark:text-emerald-400" : "text-red-500",
                          )}
                          title={b.name}
                        >
                          {b.name}
                        </span>
                        {(b.externalProcs?.length ?? 0) > 0 && (
                          <span
                            title={t("adb.binary.externalRunningTip")}
                            data-testid={`external-${b.name}`}
                            className="inline-flex h-5 shrink-0 items-center rounded-full bg-amber-500/10 px-2 text-[10px] font-medium leading-none text-amber-500"
                          >
                            {t("adb.binary.externalRunning", { pids: describeProcs(b.externalProcs) })}
                          </span>
                        )}
                      </div>
                      {/* 备注：一行文字或一个"加备注"入口，绝不换行撑高整行 */}
                      <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-muted-foreground">
                        {editingNote === b.name ? (
                          <span data-note-editor className="contents">
                            <input
                              autoFocus
                              aria-label={t("adb.binary.noteLabel", { name: b.name })}
                              data-testid={`note-${b.name}`}
                              className="h-6 min-w-0 max-w-[20rem] flex-1 rounded-md border border-input bg-background px-1.5 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                              placeholder={t("adb.binary.notePlaceholder")}
                              value={noteOf(b.name)}
                              onChange={(e) => writeNote(b.name, e.target.value)}
                              onBlur={(event) => {
                                const next = event.relatedTarget as HTMLElement | null;
                                if (next?.closest("[data-note-editor]")) return;
                                setEditingNote(null);
                              }}
                              onKeyDown={(e) => {
                                if (e.key === "Enter" || e.key === "Escape") setEditingNote(null);
                              }}
                            />
                            <Select
                              aria-label={t("adb.binary.noteQuick")}
                              data-testid={`note-quick-${b.name}`}
                              className="h-6 w-20 shrink-0 text-[11px] text-muted-foreground"
                              value=""
                              onChange={(e) => {
                                if (!e.target.value) return;
                                writeNote(b.name, e.target.value);
                                setEditingNote(null);
                              }}
                            >
                              <option value="">{t("adb.binary.noteQuick")}</option>
                              {NOTE_PRESETS.map((preset) => (
                                <option key={preset} value={preset}>
                                  {preset}
                                </option>
                              ))}
                            </Select>
                          </span>
                        ) : noteOf(b.name) ? (
                          <>
                            <span className="min-w-0 truncate" title={noteOf(b.name)}>
                              {noteOf(b.name)}
                            </span>
                            <button
                              type="button"
                              className="shrink-0 rounded p-0.5 hover:bg-accent hover:text-foreground"
                              aria-label={t("adb.binary.noteEdit")}
                              data-testid={`note-edit-${b.name}`}
                              onClick={() => setEditingNote(b.name)}
                            >
                              <Pencil className="h-3 w-3" />
                            </button>
                          </>
                        ) : (
                          <button
                            type="button"
                            className="flex shrink-0 items-center gap-1 rounded px-1 py-0.5 text-[11px] hover:bg-accent hover:text-foreground"
                            aria-label={t("adb.binary.noteAdd")}
                            data-testid={`note-add-${b.name}`}
                            onClick={() => setEditingNote(b.name)}
                          >
                            <Plus className="h-3 w-3" />
                            {t("adb.binary.noteAdd")}
                          </button>
                        )}
                      </div>
                    </div>
                    {/* 权限列：与文件列表同一个胶囊、同一个列宽刻度，绝不折行 */}
                    <span className="hidden w-col-perms shrink-0 sm:inline-flex">
                      <PermissionPill perms={b.perms} testId={`bin-perms-${b.name}`} />
                    </span>
                    {/* 大小列：右对齐 + 等宽数字，单位不换行 */}
                    <span className="w-col-size shrink-0 whitespace-nowrap text-right font-mono text-[10px] tabular-nums text-muted-foreground">
                      {b.size}
                    </span>
                    {/* 动作列：只用图标 + 悬停说明 */}
                    <span className="flex w-col-actions shrink-0 items-center justify-end gap-0.5">
                      {!b.hasExec && (
                        <Button
                          size="sm"
                          variant="ghost"
                          className="h-6 w-6 px-0 text-muted-foreground hover:text-primary"
                          aria-label={t("adb.binary.chmod")}
                          title={t("adb.binary.chmod")}
                          onClick={(event) => {
                            event.stopPropagation();
                            void chmod(b);
                          }}
                        >
                          <ShieldCheck className="h-3.5 w-3.5" />
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 w-6 px-0 text-muted-foreground hover:text-foreground"
                        aria-label={t("adb.binary.addToHost")}
                        title={t("adb.binary.dblClickAdd")}
                        data-testid={`add-${b.name}`}
                        disabled={!b.hasExec}
                        onClick={(event) => {
                          event.stopPropagation();
                          addHosted(b);
                        }}
                      >
                        <Plus className="h-3.5 w-3.5" />
                      </Button>
                    </span>
                  </div>
                </ListRow>
              ))}
            </List>
          </div>
        </section>

        {/* 下区：托管执行 */}
        <section className="flex min-h-0 flex-1 flex-col gap-1" aria-label={t("adb.binary.hostTitle")}>
          <div className="flex shrink-0 items-center justify-between gap-2">
            <PaneHeader compact icon={PlayCircle} title={t("adb.binary.hostTitle")} className="min-w-0" />
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
          <div className="pane-scroll min-h-0 flex-1 overflow-auto rounded-xl border border-border/70 bg-card shadow-card">
            {hosted.length === 0 ? (
              <p className="p-3 text-xs text-muted-foreground">{t("adb.binary.hostEmpty")}</p>
            ) : (
              <List className="text-xs">
                {hosted.map((row) => {
                  const bin = binaries.find((b) => b.name === row.name);
                  // "已经在跑"有两种：我们起的（有句柄，用既有「终止」）与别人起的（无句柄，
                  // 用下面的「停止进程」）。这一行的按钮只能有一个，否则用户不知道该点哪个。
                  const outsiders = bin?.externalProcs ?? [];
                  const firstOutsider = outsiders[0] ?? null;
                      {/*
                        这一行的启动入口：原来的「执行」（Agent 后台起 + 日志文件）已经退役，
                        改成在右半列开一块真终端把进程起起来 —— 有 tty，能输入、能 Ctrl-C、输出即时；
                        参数就是下面「参数」栏里存的同一份，两处不再各说各话。
                      */}
                  return (
                    <ListRow
                      key={row.name}
                      testId={`hosted-${row.name}`}
                      interactive={false}
                      // 这一行是纵向堆叠：文件名一行 → 动作一行 → 参数/回执/端口各一块。
                      // 必须 `stack`（flex-col）而不是 `multiline`:后者只做顶部对齐、
                      // 仍是横向容器，会把每个块压成一条竖排文字（真实炸过的形态）。
                      stack
                    >
                      {/* 第一行只有文件名：以前它跟 pid 和四五个按钮挤在一行，
                          长名字被截断、按钮也挤成一坨（用户要的"名字一行、动作一行"） */}
                      <div className="flex min-w-0 items-center gap-2">
                        <InfoChip
                          className="min-w-0 text-left"
                          label={`./${row.name}`}
                          title={t("adb.binary.copyCmd", { name: row.name })}
                          testid={`cmd-${row.name}`}
                        />
                      </div>
                      {/* 第二行：pid → 动作（终止 / 停止进程 / 终端启动、端口刷新、参数、探测帮助、移出托管区） */}
                      <div
                        className="mt-1.5 flex flex-wrap items-center gap-2"
                        data-testid={`hosted-actions-${row.name}`}
                      >
                        {row.pid !== null ? (
                          <InfoChip
                            label={`pid ${row.pid}${row.root ? " · root" : ""}`}
                            className="shrink-0"
                            testid={`pid-${row.name}`}
                          />
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
                            // 正在把这次启动登记进表时先别停：那要杀的可能是我们刚起的进程
                            disabled={probing || row.claiming}
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
                            variant="default"
                            className="h-6 shrink-0 gap-1 px-2"
                            data-testid={`hosted-terminal-${row.name}`}
                            disabled={bin?.hasExec === false}
                            title={t("adb.binary.interactiveTip")}
                            aria-label={t("adb.binary.interactive")}
                            onClick={(event) => {
                              event.stopPropagation();
                              openInteractiveTerm(row.name, row.launch.argsText);
                            }}
                          >
                            <SquareTerminal className="h-3 w-3" />
                            {t("adb.binary.launchWithTerminal")}
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
                        {/* 启动配置（参数与版本指纹）：一个开合按钮就够，默认收起 */}
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
                            <span className="text-muted-foreground">
                              {row.claiming
                                ? t("adb.binary.claiming")
                                : row.claimDetail
                                  ? t("adb.binary.claimFail", { detail: row.claimDetail })
                                  : t("adb.binary.externalRunningTip")}
                            </span>
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
                          {/* 参数写法直接给例子：是 `-a -b` 那一种（保留短横线），不是 a b */}
                          <p className="text-10px leading-snug text-muted-foreground">
                            {t("adb.binary.argsHint")}
                          </p>
                          <input
                            aria-label={t("adb.binary.argsLabel", { name: row.name })}
                            data-testid={`args-${row.name}`}
                            className="path-selectable h-6 rounded-md border border-input bg-transparent px-2 font-mono text-xs"
                            placeholder={t("adb.binary.argsPlaceholder")}
                            value={row.launch.argsText}
                            /*
                             * 全关智能替换：页面是 `lang="zh-CN"`，WebKit 在中文语境下会把连续两个
                             * ASCII `-` 当排版连字符合成 `—`，用户看到的就是"打第二个横杠时吞掉一个"。
                             * 参数栏里每个字符都是字面量，任何"智能"都是错。
                             */
                            spellCheck={false}
                            autoCorrect="off"
                            autoCapitalize="off"
                            autoComplete="off"
                            onChange={(e) => patchLaunch(row, { argsText: e.target.value })}
                          />
                          {/* 原样回显实际会发出去的 argv：看得见的才可信 */}
                          {row.launch.argsText.trim() ? (
                            <p
                              className="break-all font-mono text-10px text-muted-foreground"
                              data-testid={`args-preview-${row.name}`}
                            >
                              {t("adb.binary.argsPreview", {
                                argv: splitArgs(row.launch.argsText).join(" "),
                              })}
                            </p>
                          ) : null}
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
                            // "未知"（老记录没有修改时间）不提示：那是参数来源的元信息，
                            // 对用户没有动作可做。原来这里印一句"没做版本核对…（Legacy 来源）"，
                            // 属于调试信息出现在界面上。
                            return null;
                          })()}
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
                          onTranslate={(path) => void translateProbe(row, path)}
                          aiBaseUrl={aiConfig?.baseUrl ?? ""}
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
                      {row.actionNote && (
                        <p className="mt-1.5 break-all text-11px leading-relaxed text-muted-foreground" data-testid={`action-note-${row.name}`}>
                          {row.actionNote}
                        </p>
                      )}
                      {/* 端口：只在"确实为当前这个 pid 查过"之后才开口。
                          没查过就印一句"无 LISTEN 端口"，那是拿一次没发生的查询当证据 */}
                      {row.pid !== null && row.portsForPid === row.pid && (
                        <div className="mt-1.5 flex flex-wrap items-center gap-1.5" data-testid={`ports-${row.name}`}>
                          {row.portsError ? (
                            <span
                              className="text-10px text-destructive"
                              data-testid={`ports-error-${row.name}`}
                            >
                              {t("adb.binary.portsFail", { detail: row.portsError })}
                            </span>
                          ) : row.ports.length === 0 ? (
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
                    </ListRow>
                  );
                })}
              </List>
            )}
          </div>
        </section>
      </div>
        <div className="flex min-w-0 flex-1 flex-col">
          <HostedTerminal
            ref={termRef}
            adbPath={env?.path ?? null}
            serial={deviceSerial}
            request={termRequest}
          />
        </div>
      </div>

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
  aiBaseUrl: string;
  targetLang: string;
  t: TranslateFn;
  onAbort: (path: string[]) => void;
  onRetry: (path: string[]) => void;
  onTranslate: (path: string[]) => void;
  onConfig: () => void;
  onClose: (name: string) => void;
  canReconnect: boolean;
  onReconnect: () => void;
}) {
  const {
    name,
    node,
    aiEnabled,
    aiBaseUrl,
    aiTail,
    targetLang,
    t,
    onAbort,
    onRetry,
    onTranslate,
    onConfig,
    onClose,
    canReconnect,
    onReconnect,
  } = props;
  const path = node.path;
  const done = node.results.length;
  const pct = Math.min(100, Math.round((done / HELP_CANDIDATES.length) * 100));
  const next = HELP_CANDIDATES[Math.min(done, HELP_CANDIDATES.length - 1)];
  return (
    <div
      className="flex flex-col gap-1.5"
      data-testid={`probe-panel-${name}`}
    >
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
              {canReconnect && (
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
          {(() => {
            const verdict = judgeProbe(node.results);
            const help = verdict.kind === "help";
            const text = help ? stripAnsi(verdict.text) : "";
            return (
              <>
                <div className="flex flex-wrap items-center gap-1.5 text-10px">
                  {/* 一句话结论：有 / 没有。过程（试了哪几条、各是什么形状）不给用户看 */}
                  <span
                    className={cn(
                      "shrink-0 rounded px-1.5 py-0.5",
                      help
                        ? "bg-emerald-500/10 text-emerald-500"
                        : "bg-muted text-muted-foreground",
                    )}
                    data-testid={`probe-outcome-${name}`}
                  >
                    {help ? t("adb.binary.probeFound") : t("adb.binary.probeNotFound")}
                  </span>
                  <span className="min-w-0 flex-1" />
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
                  {help && (
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 shrink-0 gap-1 px-1.5 text-10px"
                      data-testid={`probe-translate-${name}`}
                      disabled={node.translating}
                      title={
                        aiEnabled
                          ? t("adb.binary.translateNotice", { url: aiBaseUrl })
                          : t("adb.binary.translateGoConfig")
                      }
                      onClick={() => (aiEnabled ? onTranslate(path) : onConfig())}
                    >
                      <Languages className="h-3 w-3" />
                      {node.translating
                        ? t("adb.binary.translating")
                        : aiEnabled
                          ? node.translated
                            ? t("adb.binary.translateAgain", { lang: targetLang })
                            : t("adb.binary.translate", { lang: targetLang })
                          : t("adb.binary.translateGoConfig")}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 shrink-0 gap-1 px-1.5 text-muted-foreground"
                    data-testid={`probe-config-${name}`}
                    title={t("adb.binary.translateGoConfig")}
                    onClick={onConfig}
                  >
                    <Settings2 className="h-3 w-3" />
                    {aiEnabled
                      ? t("adb.binary.translateOn", { tail: aiTail })
                      : t("adb.binary.translateOff")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 shrink-0 px-1.5 text-muted-foreground"
                    data-testid={`probe-close-${name}`}
                    title={t("adb.binary.probeClose")}
                    onClick={() => onClose(name)}
                  >
                    <X className="h-3 w-3" />
                  </Button>
                </div>
                {help ? (
                  <>
                    <pre
                      className="pane-scroll path-selectable max-h-72 overflow-auto whitespace-pre-wrap break-all rounded bg-muted/40 p-1 font-mono text-10px"
                      data-testid={`probe-text-${name}`}
                    >
                      {/* 有了译文就只留译文；判据（截断多少、有没有杀干净）另起一句 */}
                      {node.translated ?? text}
                    </pre>
                    {verdict.kind === "help" && verdict.truncated && (
                      <p className="text-10px text-amber-500" data-testid={`probe-truncated-${name}`}>
                        {t("adb.binary.probeTruncated", {
                          bytes: String(probeHiddenBytes(verdict.result)),
                        })}
                      </p>
                    )}
                    {verdict.kind === "help" && verdict.stillRunning && (
                      <p className="text-10px text-destructive" data-testid={`probe-alive-${name}`}>
                        {t("adb.binary.probeStillRunning", { pid: String(verdict.result.pid) })}
                      </p>
                    )}
                    {node.translateNote && (
                      <p className="break-all text-10px text-muted-foreground">
                        {node.translateNote}
                      </p>
                    )}
                  </>
                ) : (
                  <>
                    {verdict.detail ? (
                      <p
                        className="break-all text-10px leading-relaxed text-muted-foreground"
                        data-testid={`probe-detail-${name}`}
                      >
                        {verdict.detail}
                      </p>
                    ) : (
                      <p className="text-10px leading-relaxed text-muted-foreground">
                        {t("adb.binary.probeNotFoundHint")}
                      </p>
                    )}
                    {node.translateNote && (
                      <p className="break-all text-10px text-muted-foreground">
                        {node.translateNote}
                      </p>
                    )}
                  </>
                )}
              </>
            );
          })()}
        </div>
      )}
    </div>
  );
}
