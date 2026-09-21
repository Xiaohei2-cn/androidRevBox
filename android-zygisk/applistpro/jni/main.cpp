/*
 * ApplistPro —— 自有 Zygisk 模块 v2 基线（与 demo `applist` 并存，不改 demo 源码）
 *
 * 为什么要有这个变体（阶段文档 AR5.6 / D016~D018）：
 *   1. demo 的 TCP 11500 完全没有鉴权，设备内任意进程都能读全机清单和任意 APK；
 *      v2 增加令牌握手 + 版本/能力协商，令牌只存在于 root-only 模块目录里。
 *   2. demo 只能返回「设备当前 locale」的名称；v2 的清单命令带 locale / scope /
 *      include_disabled 参数，并把每条结果标注 labelSource / resolvedLocale / fallbackReason，
 *      解析不出来就明说，不拿包名冒充本地化名称。
 *   3. demo 一行 JSON 受 4 MiB 响应缓冲限制；v2 改成 NDJSON + 4 字节长度前缀分帧，
 *      可按包数继续增长。
 *
 * 数据流（与 demo 同构，但只服务已鉴权连接）：
 *   Android Agent (root 读 token) --TCP 127.0.0.1:11501--> root companion 服务进程
 *     -> fork+exec app_process 跑 pro.applist.HelperPro (系统级 Java 进程)
 *     -> ActivityThread.systemMain().getSystemContext().getPackageManager()
 *     -> 分帧写回
 *
 * 触发时机：system_server 在 preServerSpecialize 里 connectCompanion() 送
 *   "INIT <module_dir>\n"，点亮服务（此刻仍持 zygote 特权，SELinux 允许 bind/connect）。
 */

#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/system_properties.h>
#include <sys/types.h>
#include <signal.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <android/log.h>

#include "zygisk.hpp"

#define MODID "applistpro"
#define APPLISTPRO_PORT 11501
#define PROTOCOL_V2 2
#define MAX_HELPER_OUT (8u * 1024u * 1024u)
#define MAX_FRAME_LINES 200000
#define MAX_EXPORT_BYTES (512ull * 1024ull * 1024ull)
#define IDLE_TIMEOUT_MS 30000
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, MODID, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, MODID, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, MODID, __VA_ARGS__)

static char g_module_dir[PATH_MAX];
static char g_dex_path[PATH_MAX];
static char g_token[129];       // 64 字节随机 -> 128 hex + NUL
static char g_version[64] = "?";
static long g_version_code = 0;
static char g_locale[128] = "";
static bool service_started = false;

// ===== handler 显式注册表（AR10.2）=====
//
// 为什么不是"能跑就行"：改这里之前，`run_helper` 是**没有超时的阻塞读**，
// 而 companion 是单线程串行处理——helper 一旦卡住，整条 bridge 就不动了，
// 表现是"所有 Zygisk 能力一起消失"。阶段文档要求的是「单个 handler 崩溃/超时
// 只熔断该 method」，所以把每条命令的预算写进表里，再按表执行。
struct Handler {
    const char *cmd;          // 线协议里的命令词
    const char *capability;   // 需要宣告的能力位；nullptr = 握手后即可用
    const char *target;       // 活儿最终落在哪个进程
    const char *permission;   // 鉴权要求（这里统一是令牌握手后）
    int timeout_ms;           // **该 handler 自己**的 helper 等待上限
    size_t max_response;      // 该 handler 的响应字节上限
    bool cancellable;         // 客户端断开/X 时能否安全中断（不会留半成品）
};

// 能力位只有这一个来源：`OK`/`STATUS` 行的能力列表从这里拼。
// 改之前是两处各写一遍 "list manifest export"，加命令必然漏一处——
// 漏掉的那条对 Agent 就等于"模块没这能力"，很难看出来。
static const Handler HANDLERS[] = {
    {"S", nullptr,    "companion",            "token", 0,     512,               true},
    {"L", "list",    "system_server Java",   "token", 8000,  6u * 1024 * 1024,  true},
    {"M", "manifest","system_server Java",   "token", 8000,  6u * 1024 * 1024,  true},
    {"E", "export",  "system_server Java + 文件读", "token", 8000, 6u * 1024 * 1024, true},
    {"I", "handlers","companion",            "token", 0,     8u * 1024,         true},
};
static const size_t HANDLER_COUNT = sizeof(HANDLERS) / sizeof(HANDLERS[0]);

// 熔断：连续 3 次**内部**失败（超时/崩溃/超限）就把该方法关 60 s。
// 参数非法不计入——那是调用方的错，罚它等于让一个拼错 locale 的请求
// 把整个能力关掉，方向完全反了。
#define FUSE_THRESHOLD 3
#define FUSE_COOLDOWN_MS 60000
struct HandlerState { int fails; long long fused_until_ms; };
static HandlerState g_hstate[HANDLER_COUNT];

static long long now_ms() {
    struct timespec ts{};
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long) ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static const Handler *find_handler(const char *cmd, size_t *idx_out) {
    for (size_t i = 0; i < HANDLER_COUNT; i++) {
        if (strcmp(HANDLERS[i].cmd, cmd) == 0) {
            if (idx_out) *idx_out = i;
            return &HANDLERS[i];
        }
    }
    return nullptr;
}

static bool handler_fused(size_t idx) {
    return g_hstate[idx].fused_until_ms > now_ms();
}

static void note_internal_failure(size_t idx, const char *why) {
    if (g_hstate[idx].fails < FUSE_THRESHOLD) g_hstate[idx].fails++;
    if (g_hstate[idx].fails >= FUSE_THRESHOLD) {
        g_hstate[idx].fused_until_ms = now_ms() + FUSE_COOLDOWN_MS;
        LOGE("fuse method %s for %dms after %d failures (%s)", HANDLERS[idx].cmd,
             FUSE_COOLDOWN_MS, g_hstate[idx].fails, why);
    }
}

static void note_success(size_t idx) { g_hstate[idx].fails = 0; }

// 能力列表与「已熔断的方法」都从这里长出来，保证只有一处真来源
// 握手行与状态行共用同一个构造器：字段顺序只有一处定义，能力位从注册表长出来。
// 改之前 `OK ...` 和 `STATUS ...` 各写一遍能力列表，加命令必然漏一处——
// 漏掉的那条对 Agent 就等于「模块没这个能力」，而且很难看出来。
static int status_line(char *out, size_t cap, const char *word) {
    int n = snprintf(out, cap, "%s %d %s %ld %s", word, PROTOCOL_V2, g_version, g_version_code,
                     g_locale);
    for (size_t i = 0; i < HANDLER_COUNT; i++) {
        if (HANDLERS[i].capability && !handler_fused(i)) {
            n += snprintf(out + n, cap - (size_t) n, " %s", HANDLERS[i].capability);
        }
    }
    n += snprintf(out + n, cap - (size_t) n, "\n");
    return n;
}

// ===== 小工具：全双工写、按行读、常量时间比较 =====

static bool write_all(int fd, const void *data, size_t len) {
    const char *p = (const char *) data;
    while (len) {
        ssize_t n = write(fd, p, len);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            return false;
        }
        p += n;
        len -= (size_t) n;
    }
    return true;
}

static bool write_text(int fd, const char *s) { return write_all(fd, s, strlen(s)); }

// 读一行（含 NUL 终止，去掉 '\n'）。返回 0=EOF/错误，>0=长度
static int read_line(int fd, char *buf, size_t cap) {
    size_t n = 0;
    while (n + 1 < cap) {
        char c;
        ssize_t r = read(fd, &c, 1);
        if (r == 0) break;
        if (r < 0) {
            if (errno == EINTR) continue;
            return 0;
        }
        if (c == '\n') break;
        if (c == '\r') continue;
        buf[n++] = c;
    }
    buf[n] = '\0';
    return (int) n;
}

static bool ct_equal(const char *a, const char *b) {
    size_t la = strlen(a), lb = strlen(b);
    if (la != lb) return false;
    unsigned char diff = 0;
    for (size_t i = 0; i < la; i++) diff |= (unsigned char) (a[i] ^ b[i]);
    return diff == 0;
}

static bool wait_readable(int fd, int ms) {
    pollfd p{fd, POLLIN, 0};
    int r = poll(&p, 1, ms);
    if (r != 1) {
        if (r < 0 && errno != EINTR) LOGW("poll failed: %s", strerror(errno));
        return false;
    }
    if ((p.revents & POLLIN) == 0) {
        LOGW("poll revents=0x%x (无可读数据)", p.revents);
        return false;
    }
    return true;
}

// 4 字节大端长度前缀 + 载荷
static bool write_frame(int fd, const char *payload, size_t len) {
    unsigned char hdr[4];
    hdr[0] = (unsigned char) (len >> 24);
    hdr[1] = (unsigned char) (len >> 16);
    hdr[2] = (unsigned char) (len >> 8);
    hdr[3] = (unsigned char) len;
    return write_all(fd, hdr, 4) && write_all(fd, payload, len);
}

static bool send_err(int fd, const char *code, const char *msg) {
    char buf[512];
    int n = snprintf(buf, sizeof(buf), "ERR %s %s\n", code, msg ? msg : "");
    if (n <= 0) return false;
    return write_all(fd, buf, (size_t) n);
}

// ===== 令牌 / 模块元数据 =====

static void hex_encode(const unsigned char *in, size_t n, char *out) {
    static const char digits[] = "0123456789abcdef";
    for (size_t i = 0; i < n; i++) {
        out[i * 2] = digits[in[i] >> 4];
        out[i * 2 + 1] = digits[in[i] & 0xF];
    }
    out[n * 2] = '\0';
}

// 令牌只落在 root-only 的模块目录里：Agent 需要 `su` 才能读，第三方 App 拿不到。
// 令牌文件位置：模块目录**之外**。
//
// 放在 `/data/adb/modules/<id>/token` 时踩过一次真机坑：`ksud module install` 覆盖模块
// 目录、开机后的 companion 重新生成了令牌，几分钟后文件又不见了（模块目录里的"外来文件"
// 会被清理），表现是 v2 突然谁都进不来、Agent 一路退回 v1。令牌是运行期状态，
// 不该住在"由安装器拥有"的目录里，所以挪到 /data/adb 下一个我们自己拥有的文件，
// 并在启动时把旧位置的文件迁移过来（老设备升级不掉线）。
#define TOKEN_PATH_NEW "/data/adb/applistpro.token"
#define TOKEN_PATH_OLD_LEN (PATH_MAX)

static void token_path(char *out, size_t cap) { snprintf(out, cap, "%s", TOKEN_PATH_NEW); }

// 兼容矩阵规则 3 的实现细节：新模块也必须能被"还没升级的 Agent"使用。
// 新位置是权威，旧位置（模块目录内）镜像一份——它可能被安装器清掉，但足够让
// 只认旧路径的 Agent 在升级后的第一次会话里连上，不至于静默退回 v1。
static void mirror_token_to_module_dir(const char *token) {
    char legacy[PATH_MAX];
    snprintf(legacy, sizeof(legacy), "%s/token", g_module_dir);
    char tmp[PATH_MAX];
    snprintf(tmp, sizeof(tmp), "%s.mir", legacy);
    int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (fd < 0) return;
    if (write_all(fd, token, 128)) {
        fchmod(fd, 0600);
        close(fd);
        if (rename(tmp, legacy) != 0) unlink(tmp);
    } else {
        close(fd);
        unlink(tmp);
    }
}

static void migrate_old_token() {
    char oldp[TOKEN_PATH_OLD_LEN];
    snprintf(oldp, sizeof(oldp), "%s/token", g_module_dir);
    char newp[PATH_MAX];
    token_path(newp, sizeof(newp));
    struct stat st{};
    if (stat(newp, &st) == 0 || stat(oldp, &st) != 0) return;
    if (rename(oldp, newp) == 0) {
        chmod(newp, 0600);
        LOGI("migrated token from module dir to %s", newp);
        // 迁移后 g_token 还没读进来，ensure_token 会随后读到新位置并回写镜像
    }
}

static void ensure_token() {
    migrate_old_token();
    char path[PATH_MAX];
    token_path(path, sizeof(path));

    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd >= 0) {
        char raw[129] = {0};
        ssize_t n = read(fd, raw, 128);
        close(fd);
        if (n == 128) {
            memcpy(g_token, raw, 128);
            g_token[128] = '\0';
            LOGI("token loaded from %s", path);
            return;
        }
    }

    unsigned char rnd[64];
    int urandom = open("/dev/urandom", O_RDONLY | O_CLOEXEC);
    if (urandom < 0) {
        LOGW("open /dev/urandom failed: %s", strerror(errno));
        return;
    }
    ssize_t got = read(urandom, rnd, sizeof(rnd));
    close(urandom);
    if (got != (ssize_t) sizeof(rnd)) {
        LOGW("short read from /dev/urandom");
        return;
    }
    hex_encode(rnd, sizeof(rnd), g_token);

    // 先建同目录临时文件再 rename，避免半截令牌被读到
    char tmp[PATH_MAX];
    snprintf(tmp, sizeof(tmp), "%s.tmp", path);
    int out = open(tmp, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (out >= 0) {
        if (write_all(out, g_token, 128)) {
            fchmod(out, 0600);
            close(out);
            chmod(tmp, 0600);
            if (rename(tmp, path) == 0) {
                mirror_token_to_module_dir(g_token);
                LOGI("token generated at %s", path);
            } else {
                LOGW("rename token failed: %s", strerror(errno));
                unlink(tmp);
            }
        } else {
            close(out);
            unlink(tmp);
        }
    } else {
        LOGW("open token for write failed: %s", strerror(errno));
    }
}

static void load_module_meta() {
    char path[PATH_MAX];
    snprintf(path, sizeof(path), "%s/module.prop", g_module_dir);
    FILE *f = fopen(path, "r");
    if (!f) return;
    char line[256];
    while (fgets(line, sizeof(line), f)) {
        line[strcspn(line, "\r\n")] = '\0';
        if (strncmp(line, "version=", 8) == 0) {
            snprintf(g_version, sizeof(g_version), "%s", line + 8);
        } else if (strncmp(line, "versionCode=", 12) == 0) {
            g_version_code = strtol(line + 12, nullptr, 10);
        }
    }
    fclose(f);
}

static void detect_device_locale() {
    char value[PROP_VALUE_MAX] = {0};
    if (__system_property_get("persist.sys.locale", value) > 0 && value[0]) {
        snprintf(g_locale, sizeof(g_locale), "%s", value);
        return;
    }
    if (__system_property_get("ro.product.locale", value) > 0 && value[0]) {
        snprintf(g_locale, sizeof(g_locale), "%s", value);
        return;
    }
    snprintf(g_locale, sizeof(g_locale), "unknown");
}

// ===== app_process 环境（demo 踩过的坑：少一个 BOOTCLASSPATH 就静默退出码 0） =====

static char *g_env_buf;
static char *g_envp[512];
static int g_envc;

static void add_env(char *entry) {
    if (g_envc >= 511) return;
    for (int i = 0; i < g_envc; i++) {
        const char *eq = strchr(entry, '=');
        size_t keylen = eq ? (size_t) (eq - entry) : strlen(entry);
        if (strncmp(g_envp[i], entry, keylen) == 0 && g_envp[i][keylen] == '=') return;
    }
    g_envp[g_envc++] = entry;
}

extern char **environ;

// ART 缺 BOOTCLASSPATH / DEX2OATBOOTCLASSPATH / ANDROID_*_ROOT 任一变量时，
// app_process 会不打日志直接以退出码 0 结束（demo 踩过的坑）。
// Zygisk Next 的 companion 常在独立 pid namespace 里，看不到 zygote64，
// 但它自身 environ 已带全套 ART 变量 —— 因此两边都要收，先 zygote64 后 environ。
static void build_java_env() {
    g_env_buf = (char *) malloc(1 << 16);
    if (!g_env_buf) return;

    DIR *proc = opendir("/proc");
    if (!proc) return;
    struct dirent *de;
    while ((de = readdir(proc)) != nullptr) {
        char *end;
        long pid = strtol(de->d_name, &end, 10);
        if (pid <= 0 || *end != '\0') continue;

        char path[64], comm[64];
        snprintf(path, sizeof(path), "/proc/%ld/comm", pid);
        FILE *f = fopen(path, "re");
        if (!f) continue;
        if (fgets(comm, sizeof(comm), f) == nullptr) {
            fclose(f);
            continue;
        }
        fclose(f);
        comm[strcspn(comm, "\n")] = '\0';
        if (strcmp(comm, "zygote64") != 0) continue;

        snprintf(path, sizeof(path), "/proc/%ld/environ", pid);
        f = fopen(path, "re");
        if (!f) continue;
        size_t n = fread(g_env_buf, 1, (1 << 16) - 1, f);
        fclose(f);
        g_env_buf[n] = '\0';

        char *p = g_env_buf;
        while (p < g_env_buf + n) {
            size_t l = strlen(p);
            if (l) add_env(p);
            p += l + 1;
        }
        if (g_envc > 0) LOGI("java env: %d entries from zygote64", g_envc);
        closedir(proc);
        break;
    }

    for (char **e = environ; *e != nullptr; e++) add_env(*e);
    g_envp[g_envc] = nullptr;
    if (g_envc > 0) {
        LOGI("java env: %d effective entries (zygote64 + companion environ)", g_envc);
    } else {
        LOGW("java env: empty; app_process 会静默退出");
    }
}

// 跑一次 Java helper，返回 malloc 的 stdout（NUL 终止）；NULL=失败
enum HelperResult { HELPER_OK, HELPER_CRASH, HELPER_TIMEOUT, HELPER_OVERFLOW };

static char *run_helper(const Handler *h, char *const argv[], HelperResult *out) {
    if (out) *out = HELPER_OK;
    if (g_envc == 0) build_java_env();
    if (g_envc == 0) {
        LOGW("no usable java environment");
        if (out) *out = HELPER_CRASH;
        return nullptr;
    }

    int pipefd[2];
    if (pipe(pipefd) != 0) {
        if (out) *out = HELPER_CRASH;
        return nullptr;
    }
    fcntl(pipefd[0], F_SETFD, FD_CLOEXEC);
    fcntl(pipefd[1], F_SETFD, 0);  // 子进程要写

    pid_t pid = fork();
    if (pid < 0) {
        close(pipefd[0]);
        close(pipefd[1]);
        if (out) *out = HELPER_CRASH;
        return nullptr;
    }
    if (pid == 0) {
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        // stdin/stderr 必须显式接管：app_process 的 System.in 会撞上被 close 的 fd 号
        int devnull = open("/dev/null", O_RDWR);
        dup2(devnull, STDIN_FILENO);
        dup2(devnull, STDERR_FILENO);
        if (devnull > 2) close(devnull);

        char cp[PATH_MAX + 32];
        snprintf(cp, sizeof(cp), "-Djava.class.path=%s", g_dex_path);
        // 参数形态沿用 demo 在本机验证过的组合：class.path + /system/bin 占位 + 主类 + 业务参数
        char *args[32];
        int argc = 0;
        args[argc++] = (char *) "app_process";
        args[argc++] = cp;
        args[argc++] = (char *) "/system/bin";
        args[argc++] = (char *) "pro.applist.HelperPro";
        for (int i = 0; argv[i] != nullptr && argc < 28; i++) args[argc++] = argv[i];
        args[argc] = nullptr;
        execve("/system/bin/app_process", args, g_envp);
        _exit(127);
    }

    close(pipefd[1]);
    const size_t cap_bytes = h && h->max_response ? h->max_response : MAX_HELPER_OUT;
    char *buf = (char *) malloc(cap_bytes + 1);
    if (!buf) {
        close(pipefd[0]);
        waitpid(pid, nullptr, 0);
        if (out) *out = HELPER_CRASH;
        return nullptr;
    }
    size_t total = 0;
    bool overflow = false;
    bool timed_out = false;
    const long long deadline = now_ms() + (h && h->timeout_ms > 0 ? h->timeout_ms : 8000);
    for (;;) {
        long long left = deadline - now_ms();
        if (left <= 0) {
            timed_out = true;
            break;
        }
        struct pollfd pfd = { pipefd[0], POLLIN, 0 };
        int pr = poll(&pfd, 1, (int) (left > 1000 ? 1000 : left));
        if (pr < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (pr == 0) continue;  // 轮询到点再看总超时
        ssize_t n = read(pipefd[0], buf + total, cap_bytes - total);
        if (n < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (n == 0) break;
        total += (size_t) n;
        if (total >= cap_bytes) {
            overflow = true;
            break;
        }
    }
    close(pipefd[0]);
    if (timed_out || overflow) {
        // 卡住或超限：先把子进程收掉，再报"这一条方法失败"。
        // 不杀的话 app_process 会留在后台继续吃内存，下一次请求更慢。
        kill(pid, SIGKILL);
        int kill_status = 0;
        waitpid(pid, &kill_status, 0);
        free(buf);
        LOGE("helper %s (cmd=%s cap=%zu)", timed_out ? "TIMED OUT" : "OVERFLOWED",
             h ? h->cmd : "?", cap_bytes);
        if (out) *out = timed_out ? HELPER_TIMEOUT : HELPER_OVERFLOW;
        return nullptr;
    }
    int status = 0;
    waitpid(pid, &status, 0);

    buf[total] = '\0';
    bool clean_exit = WIFEXITED(status) && WEXITSTATUS(status) == 0;
    LOGI("helper status=%d clean=%d out=%zu", status, clean_exit, total);
    // 「正常退出且输出为空」是合法结果（例如 --files 查不存在的包），
    // 不能和「崩了/没起来」混成一类，否则调用方拿不到 no_files 这种可诊断错误。
    if (total == 0 && !clean_exit) {
        LOGW("helper produced nothing");
        free(buf);
        if (out) *out = HELPER_CRASH;
        return nullptr;
    }
    return buf;
}

// 把 helper 的 NDJSON 逐帧发给已鉴权客户端
static bool stream_ndjson(int cfd, char *payload) {
    bool ok = true;
    char *line = payload;
    unsigned lines = 0;
    while (line && *line) {
        char *nl = strchr(line, '\n');
        if (nl) *nl = '\0';
        if (*line) {
            if (strncmp(line, "{\"error\":", 9) == 0) {
                send_err(cfd, "helper_failed", line + 9);
                ok = false;
                break;
            }
            ok = write_frame(cfd, line, strlen(line));
            if (++lines > MAX_FRAME_LINES) {
                send_err(cfd, "too_many_items", nullptr);
                ok = false;
                break;
            }
            if (!ok) break;
        }
        if (!nl) break;
        line = nl + 1;
    }
    free(payload);
    return ok;
}

static bool stream_file(int cfd, const char *path, off_t size) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return false;
    char chunk[64 * 1024];
    off_t left = size;
    while (left > 0) {
        size_t want = (size_t) (left > (off_t) sizeof(chunk) ? sizeof(chunk) : left);
        ssize_t n = read(fd, chunk, want);
        if (n <= 0) {
            close(fd);
            return false;
        }
        if (!write_all(cfd, chunk, (size_t) n)) {
            close(fd);
            return false;
        }
        left -= n;
    }
    close(fd);
    return true;
}

static bool cmd_export(int cfd, const Handler *h, size_t hidx, char *const argv[]) {
    // 命中数在此统计：一个文件都没匹配上时显式 ERR，而不是回空 DONE
    HelperResult hr = HELPER_OK;
    char *payload = run_helper(h, argv, &hr);
    if (!payload) {
        if (hr != HELPER_OK) note_internal_failure(hidx, "export helper");
        return send_err(cfd, hr == HELPER_TIMEOUT ? "helper_timeout" : "helper_failed", nullptr);
    }
    note_success(hidx);

    unsigned long long total = 0;
    char *line = payload;
    while (line && *line) {
        char *nl = strchr(line, '\n');
        if (nl) *nl = '\0';
        char *tab = strchr(line, '\t');
        if (tab) {
            *tab = '\0';
            const char *pkg = line;
            const char *path = tab + 1;
            bool want = strcmp(pkg, argv[1]) == 0;
            if (want) {
                struct stat st{};
                if (stat(path, &st) == 0 && st.st_size > 0) {
                    if ((unsigned long long) st.st_size > MAX_EXPORT_BYTES ||
                        total + (unsigned long long) st.st_size > MAX_EXPORT_BYTES) {
                        const char *base = strrchr(path, '/');
                        char hdr[PATH_MAX + 64];
                        int n = snprintf(hdr, sizeof(hdr), "T %lld %s %s too_large\n",
                                         (long long) st.st_size, pkg, base ? base + 1 : path);
                        write_all(cfd, hdr, (size_t) n);
                    } else {
                        const char *base = strrchr(path, '/');
                        base = base ? base + 1 : path;
                        char hdr[PATH_MAX * 2 + 64];
                        int n = snprintf(hdr, sizeof(hdr), "F %lld %s %s\n",
                                         (long long) st.st_size, pkg, base);
                        if (!write_all(cfd, hdr, (size_t) n) ||
                            !stream_file(cfd, path, st.st_size)) {
                            free(payload);
                            return false;
                        }
                        total += (unsigned long long) st.st_size;
                    }
                }
            }
        }
        if (!nl) break;
        line = nl + 1;
    }
    free(payload);
    if (total == 0) {
        return send_err(cfd, "no_files", argv[1]);
    }
    return write_text(cfd, "DONE\n");
}

// 已鉴权后的命令循环：命令词 → 注册表条目 → 按条目声明的预算与熔断状态执行。
//
// 三条不变式：
// 1. 参数非法永远只回 `bad_*`，**不计入熔断**（调用方的错不该让能力下线）；
// 2. 内部失败（超时/崩溃/超限）计入熔断，且只关这一条方法，其余方法继续服务；
// 3. 熔断期间该方法不再出现在能力列表里（Agent 侧会看到 capability_missing
//    而不是超时），冷却窗口过后自动回来。
static void serve_session(int cfd) {
    char line[512];
    for (;;) {
        if (!wait_readable(cfd, IDLE_TIMEOUT_MS)) {
            LOGI("session idle timeout");
            return;
        }
        int got = read_line(cfd, line, sizeof(line));
        if (got == 0) {
            LOGI("session eof");
            return;
        }
        if (strncmp(line, "H ", 2) == 0) line[4] = '\0';  // 不在日志里留令牌
        LOGI("cmd %s", line);

        if (strcmp(line, "X") == 0) return;

        char cmd[8] = "";
        char rest[512 - 8];
        if (sscanf(line, "%7s %503[^\n]", cmd, rest) < 1) {
            send_err(cfd, "bad_request", line);
            continue;
        }
        size_t hidx = 0;
        const Handler *h = find_handler(cmd, &hidx);
        if (!h) {
            send_err(cfd, "unknown_command", line);
            continue;
        }
        if (handler_fused(hidx)) {
            send_err(cfd, "method_fused", h->cmd);
            continue;
        }

        if (strcmp(h->cmd, "S") == 0) {
            char body[512];
            status_line(body, sizeof(body), "STATUS");
            write_text(cfd, body);
            continue;
        }

        if (strcmp(h->cmd, "I") == 0) {
            // 注册表自描述：排障时能直接问"模块认为自己有哪些方法、哪个正在熔断"。
            // 能力名 handlers 对旧 Agent 是纯加法（不认识就忽略），见兼容矩阵规则 3。
            // stream_ndjson 负责释放，所以这里必须是堆内存。
            char *payload = (char *) malloc(8u * 1024);
            if (!payload) {
                send_err(cfd, "helper_failed", "oom");
                continue;
            }
            int off = 0;
            for (size_t i = 0; i < HANDLER_COUNT; i++) {
                char capjson[64] = "null";
                if (HANDLERS[i].capability) {
                    snprintf(capjson, sizeof(capjson), "\"%s\"", HANDLERS[i].capability);
                }
                int wrote = snprintf(payload + off, sizeof(char) * (8u * 1024 - (size_t) off),
                                     "{\"cmd\":\"%s\",\"capability\":%s,\"target\":\"%s\","
                                     "\"permission\":\"%s\",\"timeout_ms\":%d,"
                                     "\"max_response_bytes\":%zu,\"cancellable\":%s,"
                                     "\"fused\":%s}\n",
                                     HANDLERS[i].cmd, capjson, HANDLERS[i].target,
                                     HANDLERS[i].permission, HANDLERS[i].timeout_ms,
                                     HANDLERS[i].max_response,
                                     HANDLERS[i].cancellable ? "true" : "false",
                                     handler_fused(i) ? "true" : "false");
                if (wrote < 0 || off + wrote >= (int) (8u * 1024)) break;
                off += wrote;
            }
            stream_ndjson(cfd, payload);
            continue;
        }

        if (strcmp(h->cmd, "L") == 0) {
            char locale[128] = "-", scope[16] = "all";
            int include_disabled = 0;
            if (sscanf(rest, "%127s %15s %d", locale, scope, &include_disabled) < 2) {
                send_err(cfd, "bad_request", "L <locale|-> <all|user|system> <0|1>");
                continue;
            }
            if (strcmp(locale, "-") != 0) {
                size_t llen = strlen(locale);
                bool ok = llen > 0 && llen <= 35;
                for (size_t i = 0; i < llen && ok; i++) {
                    char c = locale[i];
                    ok = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') ||
                         c == '-' || c == '_' || c == '.';
                }
                if (!ok) {
                    send_err(cfd, "bad_locale", locale);
                    continue;
                }
            }
            if (strcmp(scope, "all") && strcmp(scope, "user") && strcmp(scope, "system")) {
                send_err(cfd, "bad_scope", scope);
                continue;
            }
            char disabled[8];
            snprintf(disabled, sizeof(disabled), "%d", include_disabled ? 1 : 0);
            char *argv[] = {(char *) "--list", locale, scope, disabled, nullptr};
            HelperResult hr = HELPER_OK;
            char *payload = run_helper(h, argv, &hr);
            if (!payload) {
                if (hr != HELPER_OK) note_internal_failure(hidx, "list helper");
                send_err(cfd, hr == HELPER_TIMEOUT ? "helper_timeout" : "helper_failed", nullptr);
            } else {
                note_success(hidx);
                stream_ndjson(cfd, payload);
            }
            continue;
        }

        if (strcmp(h->cmd, "M") == 0) {
            char *argv[] = {(char *) "--manifest", nullptr};
            HelperResult hr = HELPER_OK;
            char *payload = run_helper(h, argv, &hr);
            if (!payload) {
                if (hr != HELPER_OK) note_internal_failure(hidx, "manifest helper");
                send_err(cfd, hr == HELPER_TIMEOUT ? "helper_timeout" : "helper_failed", nullptr);
            } else {
                note_success(hidx);
                stream_ndjson(cfd, payload);
            }
            continue;
        }

        if (strcmp(h->cmd, "E") == 0) {
            const char *pkg = rest;
            if (*pkg == '\0' || strpbrk(pkg, "/\\ \"'`$;&|<>()") != nullptr || strlen(pkg) > 256) {
                send_err(cfd, "bad_package", pkg);
                continue;
            }
            char *argv[] = {(char *) "--files", (char *) pkg, nullptr};
            if (!cmd_export(cfd, h, hidx, argv)) return;
            continue;
        }

        send_err(cfd, "unknown_command", line);
    }
}

// 以磁盘为准刷新令牌（模块升级路径的真问题）：
// `ksud module install` 会覆盖模块目录，此时**还在跑的 companion 手里握的是已经从磁盘
// 上消失的旧令牌**，而 Agent 只能读文件——结果 v2 谁都进不来，直到有人重启手机。
// 所以每次握手前先照一遍文件：文件在就用文件里的；不在就重新生成一份并落盘。
// 代价是每条握手多一次 128 字节读，换掉的是"装完模块必须重启才能用"。
static void reload_token() {
    migrate_old_token();
    char path[PATH_MAX];
    token_path(path, sizeof(path));
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd >= 0) {
        char raw[129] = {0};
        ssize_t n = read(fd, raw, 128);
        close(fd);
        if (n == 128 && memcmp(raw, g_token, 128) != 0) {
            memcpy(g_token, raw, 128);
            g_token[128] = '\0';
            mirror_token_to_module_dir(g_token);
            LOGI("token reloaded from disk (module upgraded?)");
            return;
        }
        if (n == 128) return;  // 与内存一致，什么都不做
    }
    LOGW("token file missing/unreadable, regenerating");
    ensure_token();
}

// 未鉴权前只接受 3 次握手尝试；令牌不匹配直接断开（不泄露原因细节）
static bool authenticate(int cfd) {
    for (int attempt = 0; attempt < 3; attempt++) {
        char line[256];
        if (!wait_readable(cfd, 5000)) return false;
        if (read_line(cfd, line, sizeof(line)) == 0) return false;
        if (strncmp(line, "H ", 2) != 0) {
            send_err(cfd, "auth_required", nullptr);
            continue;
        }
        int proto = 0;
        char token[160] = {0};
        if (sscanf(line, "H %d %159s", &proto, token) != 2) {
            send_err(cfd, "bad_handshake", nullptr);
            continue;
        }
        if (proto != PROTOCOL_V2) {
            send_err(cfd, "unsupported_protocol", nullptr);
            return false;
        }
        reload_token();
        if (g_token[0] == '\0' || !ct_equal(g_token, token)) {
            LOGW("auth rejected");
            send_err(cfd, "auth_failed", nullptr);
            continue;
        }
        char body[512];
        int n = status_line(body, sizeof(body), "OK");
        if (!write_all(cfd, body, (size_t) n)) return false;
        return true;
    }
    return false;
}

// 常驻服务：只绑 127.0.0.1，必须经 adb forward / 本机进程访问；单线程串行处理，
// 避免多个 helper 冷启动互相踩踏（一次查询约 0.4s）。
static void serve_forever() {
    ensure_token();
    // 服务点亮就立刻镜像一份到旧位置：只新生成时镜像会漏掉"从磁盘加载"这条路径，
    // 而还没升级的 Agent 只认旧路径——它读不到令牌就从不连接，也就永远不会触发 reload。
    if (g_token[0] != '\0') mirror_token_to_module_dir(g_token);
    load_module_meta();
    detect_device_locale();
    if (g_token[0] == '\0') {
        LOGW("no token available, refusing to serve");
        return;
    }

    int lfd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (lfd < 0) _exit(1);
    int opt = 1;
    setsockopt(lfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = htons(APPLISTPRO_PORT);
    if (bind(lfd, (sockaddr *) &addr, sizeof(addr)) != 0) _exit(2);
    if (listen(lfd, 4) != 0) _exit(3);
    LOGI("serving on 127.0.0.1:%d proto=%d", APPLISTPRO_PORT, PROTOCOL_V2);

    for (;;) {
        int cfd = accept4(lfd, nullptr, nullptr, SOCK_CLOEXEC);
        if (cfd < 0) {
            LOGW("accept failed: %s", strerror(errno));
            continue;
        }
        bool authed = authenticate(cfd);
        if (authed) serve_session(cfd);
        LOGI("session closed authed=%d", authed);
        close(cfd);
    }
}

static void companion_handler(int sock) {
    char line[PATH_MAX + 8];
    ssize_t len = read(sock, line, sizeof(line) - 1);
    if (len > 0) {
        line[len] = '\0';
        if (strncmp(line, "INIT ", 5) == 0 && !service_started) {
            char *dir = line + 5;
            char *nl = strchr(dir, '\n');
            if (nl) *nl = '\0';
            snprintf(g_module_dir, sizeof(g_module_dir), "%s", dir);
            snprintf(g_dex_path, sizeof(g_dex_path), "%s/helper.dex", dir);
            service_started = true;
            if (fork() == 0) {
                serve_forever();
                _exit(0);
            }
        }
    }
    close(sock);
}

class ApplistProModule final : public zygisk::ModuleBase {
public:
    void onLoad(zygisk::Api *api, JNIEnv *env) override {
        this->api = api;
        this->env = env;
    }

    // 本模块不注入任何普通 App：立刻卸载自己，不在无关进程留痕迹。
    void preAppSpecialize(zygisk::AppSpecializeArgs *args) override {
        api->setOption(zygisk::Option::DLCLOSE_MODULE_LIBRARY);
    }

    // system_server 刚 fork、还没 specialize：此刻仍有 zygote 特权。
    // connectCompanion() / getModuleDir() 只能在 pre 阶段调用（SELinux 限制），
    // 且这里绝不做枚举或阻塞等待，只把模块目录交给 companion 点亮服务。
    void preServerSpecialize(zygisk::ServerSpecializeArgs *args) override {
        int fd = api->connectCompanion();
        if (fd >= 0) {
            char link[64], modpath[PATH_MAX];
            int dirfd = api->getModuleDir();
            snprintf(link, sizeof(link), "/proc/self/fd/%d", dirfd);
            ssize_t len = readlink(link, modpath, sizeof(modpath) - 1);
            if (len > 0) {
                modpath[len] = '\0';
                char req[PATH_MAX + 8];
                int rlen = snprintf(req, sizeof(req), "INIT %s\n", modpath);
                if (write(fd, req, (size_t) rlen) < 0) {
                    LOGW("INIT write failed: %s", strerror(errno));
                }
            } else {
                LOGW("readlink module dir failed: %s", strerror(errno));
            }
            close(fd);
        } else {
            LOGW("connectCompanion failed");
        }
        api->setOption(zygisk::Option::DLCLOSE_MODULE_LIBRARY);
    }

private:
    zygisk::Api *api = nullptr;
    JNIEnv *env = nullptr;
};

REGISTER_ZYGISK_MODULE(ApplistProModule)
REGISTER_ZYGISK_COMPANION(companion_handler)
