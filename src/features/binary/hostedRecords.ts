/**
 * 本机（localStorage）里那两类「按设备 + 文件」存的记录：文件备注、启动参数。
 *
 * 为什么单独一个文件，而不是顺手写在页面里：
 * 1. 这是**会删用户数据**的一段逻辑，必须能脱离界面被钉住（真删数据的东西不该只靠手点验证）；
 * 2. 全站只有这里知道键的格式。写的一方与清的一方如果各拼各的前缀，
 *    那就是"清错设备"的开始——用户明确要求**以设备号对应、各台手机互不相干**。
 *
 * 键形状（`.` 分隔，序列号本身不含点号后再接另一个序列号这种形状：USB 序列号是字母数字，
 * 网络调试地址是 `ip:port`）：
 * - 备注：`adb.binary.note.<serial>.<name>`
 * - 启动参数：`adb.binary.launch.<serial>.<name>.args` / `.stamp`
 */
export const NOTE_PREFIX = "adb.binary.note.";
export const LAUNCH_PREFIX = "adb.binary.launch.";

/** 备注的键（唯一出口：写与清都走这里） */
export const noteKey = (serial: string, name: string) => `${NOTE_PREFIX}${serial}.${name}`;
/** 启动参数的键前缀（`.args` / `.stamp` 由调用方补上） */
export const launchKeyOf = (serial: string, name: string) => `${LAUNCH_PREFIX}${serial}.${name}`;

/** 只要求这几个能力：真 localStorage 与测试里的假存储都满足 */
export type RecordStorage = Pick<Storage, "length" | "key" | "removeItem">;

const forEachKey = (storage: RecordStorage, run: (key: string) => void) => {
  // 反向遍历：删除会让 jsdom/浏览器把后面的键往前挪，正着走会跳过条目
  for (let i = storage.length - 1; i >= 0; i -= 1) {
    const key = storage.key(i);
    if (key) run(key);
  }
};

/** 这台设备的一个键属于哪个文件名；不属于这台设备返回 null */
function nameOfKey(key: string, serial: string): string | null {
  const prefix = `${NOTE_PREFIX}${serial}.`;
  if (key.startsWith(prefix)) return key.slice(prefix.length);
  const launchPrefix = `${LAUNCH_PREFIX}${serial}.`;
  if (!key.startsWith(launchPrefix)) return null;
  const rest = key.slice(launchPrefix.length);
  // 参数键固定以 .args / .stamp 结尾；其余形状（脏键）不当成任何文件的记录，留着不动
  if (rest.endsWith(".args")) return rest.slice(0, -".args".length);
  if (rest.endsWith(".stamp")) return rest.slice(0, -".stamp".length);
  return null;
}

/**
 * 列出这台设备上"本机有记录、但清单里没有"的文件名（只报告，不删除）。
 * `present` 必须是**这台设备此刻的完整清单**——调用方负责保证它完整（见 BinaryHosting 那三条件）。
 */
export function orphanRecordNames(
  storage: RecordStorage,
  serial: string,
  present: ReadonlySet<string>,
): string[] {
  const found = new Set<string>();
  forEachKey(storage, (key) => {
    const name = nameOfKey(key, serial);
    if (name && !present.has(name)) found.add(name);
  });
  return [...found].sort();
}

/**
 * 删掉 `gone` 里这些文件在本机的记录，返回删掉的**键**列表（界面用它说清"清了几条"）。
 *
 * 参数是**已确认没了的文件名**，不是"当前清单"：判断谁没了是调用方的事（要拿完整清单
 * + 逐个回问设备，见 BinaryHosting），这里只负责"照着名单动手、只动这一台设备的键"。
 * 别的手机、别的 kind 一概不碰。
 */
export function pruneHostedRecords(
  storage: RecordStorage,
  serial: string,
  gone: ReadonlySet<string>,
): string[] {
  const removed: string[] = [];
  forEachKey(storage, (key) => {
    const name = nameOfKey(key, serial);
    if (!name || !gone.has(name)) return;
    storage.removeItem(key);
    removed.push(key);
  });
  return removed;
}

/**
 * 这份清单能不能当作"这台手机的文件全貌"来用（决定要不要清理本机记录）。
 *
 * 三条硬条件，缺一不可：
 * - `source === "agent"`：回退腿是 `ls -l` + `file` 拼出来的，它看不见"读全了没有"，
 *   把它的空结果当"目录里没这个文件"就会删错东西；
 * - 没被设备端上限截断、也没有读不出来的条目：截断的那一部分确实没露面；
 * - 清单非空：一个文件都没有的托管目录，要么是刚清过、要么是读崩了——两者都不该删用户记录。
 */
export function listingIsTrustworthy(listing: {
  source: string;
  truncated: boolean;
  unreadable: string[];
  binaries: unknown[];
}): boolean {
  return (
    listing.source === "agent" &&
    !listing.truncated &&
    listing.unreadable.length === 0 &&
    listing.binaries.length > 0
  );
}

/**
 * 界面该盯着哪台设备：`selected` 是用户当前选中的序列号，`online` 是 adb 这次报的在线列表。
 *
 * 为什么写成纯函数：这段判断决定"要不要扔掉用户正在用的那块终端屏"，
 * 拿整页渲染去试它的所有分支既慢又测不全。规则只有两条，都是真机上踩出来的：
 *
 * - **adb 明确报了别的设备在线**才改选（换手机之后界面继续抱着旧序列号，
 *   就会对着不存在的目标发命令，报 `adb: device '…' not found`）；
 * - 一瞬间枚举到**空列表不改选**：换线/adb 重启的空窗期把用户正在用的会话收掉，
 *   比暂时读不到清单更糟。
 */
export function pickTrackedDevice(
  selected: string | null,
  online: readonly string[],
): { serial: string | null; switchedAway: boolean } {
  if (online.length === 0) return { serial: selected, switchedAway: false };
  if (selected && online.includes(selected)) return { serial: selected, switchedAway: false };
  return { serial: online[0] as string, switchedAway: selected !== null };
}
