/*
 * applist —— Zygisk 学习模块
 *
 * 需求: 在 PC 上通过一条命令拿到手机上所有已安装 App 的"包名 + 中文显示名"。
 * label 存在每个 APK 的 resources.arsc 里, adb/pm/dumpsys 都不吐,
 * 只有进程内拿 PackageManager.getApplicationLabel() 才解析得出来 —— 这正是 Zygisk 的用武之地。
 *
 * 完整链路:
 *   PC --adb forward tcp:11500--> TCP 127.0.0.1:11500
 *     -> root companion 侧的常驻服务循环 (Magisk/KernelSU 以 root 启动的守护进程)
 *       -> 每次请求 fork+exec 一个 app_process, 跑 demo.applist.Helper (Java)
 *         -> ActivityThread.systemMain().getSystemContext().getPackageManager()
 *           -> getInstalledApplications + getApplicationLabel
 *
 * 生命周期钩子 (对照内核模块的 init/exit):
 *   REGISTER_ZYGISK_MODULE(X)   生成 zygisk_module_entry   —— 模块装载入口
 *   onLoad()                    注入目标进程后第一时间调用
 *   preAppSpecialize()          每个 App 进程隔离前 (尚有 zygote 权限)
 *   preServerSpecialize()       system_server 隔离前, 本模块在这里 connectCompanion
 *   DLCLOSE_MODULE_LIBRARY      主动卸载自己 (相当于模块 exit 的时机)
 */

#include <unistd.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <stddef.h>
#include <limits.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/sendfile.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <android/log.h>

#include "zygisk.hpp"

using zygisk::Api;
using zygisk::AppSpecializeArgs;
using zygisk::ServerSpecializeArgs;

#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, "ApplistModule", __VA_ARGS__)
#define LOGD(...) __android_log_print(ANDROID_LOG_DEBUG, "ApplistModule", __VA_ARGS__)

/* PC 接入端口: adb forward tcp:11500 tcp:11500 */
#define APPLIST_PORT 11500

/*
 * 极简文本协议 (学习用, 一层就够):
 * 客户端: 发 1 字节命令, 'Q' = 查询全部应用列表.
 * 服务端: 回一行 JSON: [{"pkg":"com.tencent.mm","label":"微信"}, ...]\n
 */

// ---------------------------------------------------------------------------
// 第一部分: 注入进各进程的模块类 —— 保持极轻, 干完活立刻 dlclose 自己
// ---------------------------------------------------------------------------

class ApplistModule : public zygisk::ModuleBase {
public:
    void onLoad(Api *api, JNIEnv *env) override {
        this->api = api;
        this->env = env;
    }

    // 每个普通 App 进程 fork 后都会走到这里. 本模块不需要注入任何 App:
    // 立刻卸载自己, 不在无关进程留痕迹.
    void preAppSpecialize(AppSpecializeArgs *args) override {
        api->setOption(zygisk::Option::DLCLOSE_MODULE_LIBRARY);
    }

    // system_server 刚 fork、还未 specialize, 此刻还有 zygote 特权,
    // connectCompanion()/getModuleDir() 只能在这种 pre 阶段调用 (SELinux 限制).
    void preServerSpecialize(ServerSpecializeArgs *args) override {
        int fd = api->connectCompanion();
        if (fd >= 0) {
            // companion 是独立进程, 不知道模块装在哪个路径.
            // 通过 getModuleDir() 拿到目录 fd, 再用 /proc/self/fd 还原真实路径, 发给它.
            char link[64], modpath[PATH_MAX];
            int dirfd = api->getModuleDir();
            snprintf(link, sizeof(link), "/proc/self/fd/%d", dirfd);
            ssize_t len = readlink(link, modpath, sizeof(modpath) - 1);
            if (len > 0) {
                modpath[len] = '\0';
                char req[PATH_MAX + 8];
                int rlen = snprintf(req, sizeof(req), "INIT %s\n", modpath);
                write(fd, req, rlen);
            }
            close(fd);
        } else {
            LOGE("connectCompanion failed");
        }

        // 我们不在 system_server 里 hook 任何东西, 同样卸载自己, 保持进程干净.
        api->setOption(zygisk::Option::DLCLOSE_MODULE_LIBRARY);
    }

private:
    Api *api;
    JNIEnv *env;
};

// ---------------------------------------------------------------------------
// 第二部分: root companion —— 真正有权限干活的守护侧
// ---------------------------------------------------------------------------
//
// REGISTER_ZYGISK_COMPANION 注册的 handler 运行在独立的 root 进程里.
// 每当有人 connectCompanion(), 就被调用一次, 参数是 socketpair 的 companion 端.
// (注意: 可被多线程并发调用, 所以用 service_started 防重复起服务.)

static char g_dex_path[PATH_MAX];            // helper.dex 绝对路径, 由 INIT 握手写入
static bool service_started = false;

// ---------------------------------------------------------------------------
// 构建 Java 运行时环境
// app_process 需要 BOOTCLASSPATH/DEX2OATBOOTCLASSPATH 和 APEX root 变量
// (ANDROID_ART_ROOT 等), companion 自己的环境里没有这些, ART 会静默死掉.
// 最可靠的来源: 直接从 zygote64 进程 /proc/<pid>/environ 复制一份,
// 它本来就是 init 拉起的标准 Java 进程环境.
// ---------------------------------------------------------------------------
#include <dirent.h>

static char *g_java_envp[64];
static int g_java_envc = 0;

static void add_env(const char *entry) {
    // 跳过 socket fd 继承变量, 其它照单全收
    if (strncmp(entry, "ANDROID_SOCKET_", 15) == 0) return;
    if (g_java_envc < 60)
        g_java_envp[g_java_envc++] = strdup(entry);
}

static void build_java_env() {
    DIR *proc = opendir("/proc");
    if (!proc) return;
    struct dirent *de;
    while ((de = readdir(proc)) != nullptr) {
        // 只找数字目录(pid), 且 comm == zygote64
        char *end;
        long pid = strtol(de->d_name, &end, 10);
        if (pid <= 0 || *end != '\0') continue;

        char path[64], comm[64];
        snprintf(path, sizeof(path), "/proc/%ld/comm", pid);
        FILE *f = fopen(path, "re");
        if (!f) continue;
        if (fgets(comm, sizeof(comm), f) == nullptr) { fclose(f); continue; }
        fclose(f);
        comm[strcspn(comm, "\n")] = '\0';
        if (strcmp(comm, "zygote64") != 0) continue;

        // 读 environ: '\0' 分隔的 KEY=VALUE 串
        snprintf(path, sizeof(path), "/proc/%ld/environ", pid);
        f = fopen(path, "re");
        if (!f) continue;
        static char envbuf[1 << 16];
        size_t n = fread(envbuf, 1, sizeof(envbuf) - 1, f);
        fclose(f);
        envbuf[n] = '\0';

        char *p = envbuf;
        while (p < envbuf + n) {
            size_t l = strlen(p);
            if (l) add_env(p);
            p += l + 1;
        }
        closedir(proc);
        if (g_java_envc > 0) {
            g_java_envp[g_java_envc] = nullptr;
            LOGD("build_java_env: %d entries from zygote64", g_java_envc);
            return;
        }
        return;
    }
    closedir(proc);
}

// fork+exec 一个 app_process 跑 Java helper, 收集它 stdout 打的一行 JSON.
// 独立 Java 进程没有 targetSdk, hidden API 检查跳过, 所以 helper 里
// 反射 ActivityThread.systemMain() 这类 @hide API 可以直接用 (官方 pm/am 同理).
#define RESP_MAX (1 << 22)   // 4MB, 足够容纳上千条应用记录

static char *query_helper(const char *arg) {
    static char resp[RESP_MAX];
    resp[0] = '\0';

    // 第一次被调用时, 从 zygote64 复制 Java 运行时环境
    static bool env_ready = false;
    if (!env_ready) {
        env_ready = true;
        build_java_env();
    }

    int pipefd[2];
    if (pipe(pipefd) != 0) return nullptr;

    pid_t pid = fork();
    if (pid < 0) {
        close(pipefd[0]);
        close(pipefd[1]);
        return nullptr;
    }
    if (pid == 0) {
        // 子进程: 变成 Java 进程, 加载 dex 执行 demo.applist.Helper.main()
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        // 关键: stdin 必须显式接管, 否则 app_process 的 System.in
        // 会撞上刚被 close 的 socketpair fd 号 (fd 复用), 输出异常
        int devnull = open("/dev/null", O_RDWR);
        dup2(devnull, STDIN_FILENO);
        dup2(devnull, STDERR_FILENO);
        if (devnull > 2) close(devnull);

        char cp[PATH_MAX + 32];
        snprintf(cp, sizeof(cp), "-Djava.class.path=%s", g_dex_path);
        // arg 可为 nullptr(Q) / "--manifest"(E) / "--files"(D 的内部数据源)
        char *argv[6];
        int argc = 0;
        argv[argc++] = (char *) "app_process";
        argv[argc++] = cp;
        argv[argc++] = (char *) "/system/bin";         // 传统 start-dir 占位参数
        argv[argc++] = (char *) "demo.applist.Helper"; // 主类
        if (arg) argv[argc++] = (char *) arg;
        argv[argc] = nullptr;
        // 环境来自 zygote64 (含 BOOTCLASSPATH/APEX root 等 ART 必需变量)
        extern char **environ;
        char **envp = g_java_envc ? g_java_envp : environ;
        execve("/system/bin/app_process", argv, envp);
        _exit(127);
    }

    close(pipefd[1]);
    size_t off = 0;
    ssize_t n;
    while (off < sizeof(resp) - 1 &&
           (n = read(pipefd[0], resp + off, sizeof(resp) - 1 - off)) > 0)
        off += n;
    resp[off] = '\0';
    close(pipefd[0]);
    int status = 0;
    waitpid(pid, &status, 0);
    LOGD("helper done: off=%zu status=%d exit=%d", off, status,
         WIFEXITED(status) ? WEXITSTATUS(status) : -1);
    return off > 0 ? resp : nullptr;
}

// 读一行 (以 \n 结尾, 不含 \n). 返回长度, 0 表示 EOF/出错.
static ssize_t read_line(int fd, char *buf, size_t cap) {
    size_t off = 0;
    while (off + 1 < cap) {
        ssize_t n = read(fd, buf + off, 1);
        if (n <= 0) break;
        if (buf[off] == '\n') break;
        off++;
    }
    buf[off] = '\0';
    return (ssize_t) off;
}

// 把文件 len 字节流式写入 socket (零拷贝优先, 失败退回用户态循环)
static bool stream_file(int cfd, const char *path, long long len) {
    int f = open(path, O_RDONLY | O_CLOEXEC);
    if (f < 0) return false;
    bool ok = true;
    long long left = len;
    off_t off = 0;
    while (left > 0) {
        ssize_t n = sendfile(cfd, f, &off, (size_t) left);
        if (n <= 0) { ok = false; break; }
        left -= n;
    }
    close(f);
    return ok;
}

// D 命令: 流式导出. 数据源是 helper --files 的 "包名\t路径" 行表.
// 协议: 每个文件前发一行 "F <size> <pkg> <name>\n" (name 为 basename),
//       随后裸流 size 字节; 全部发完再发一行 "DONE\n".
//       (包名/文件名都不含空格, 所以空格分隔安全)
static void export_files(int cfd, const char *want_pkgs[], int npkg, bool all) {
    char *tab = query_helper("--files");   // 复用 helper 输出缓冲 (4MB 上限)
    if (!tab) {
        const char *e = "ERR helper failed\n";
        write(cfd, e, strlen(e));
        return;
    }

    int sent = 0;
    char *line = tab;
    while (*line) {
        char *nl = strchr(line, '\n');
        if (!nl) break;
        *nl = '\0';
        char *tabpos = strchr(line, '\t');
        if (tabpos) {
            *tabpos = '\0';
            const char *pkg = line;
            const char *path = tabpos + 1;
            // 匹配过滤: all=true 时全导
            bool match = all;
            for (int k = 0; !match && k < npkg; k++)
                match = strcmp(pkg, want_pkgs[k]) == 0;
            if (match) {
                struct stat st;
                if (stat(path, &st) == 0 && st.st_size > 0) {
                    const char *base = strrchr(path, '/');
                    base = base ? base + 1 : path;
                    char hdr[2 * PATH_MAX + 64];
                    int hlen = snprintf(hdr, sizeof(hdr), "F %lld %s %s\n",
                                        (long long) st.st_size, pkg, base);
                    write(cfd, hdr, hlen);
                    if (stream_file(cfd, path, st.st_size))
                        sent++;
                }
            }
        }
        line = nl + 1;
    }
    const char *done = "DONE\n";
    write(cfd, done, strlen(done));
    LOGD("export done: %d files", sent);
}

// 常驻服务循环: 只在 companion fork 出的服务子进程里跑, 永不退出.
// 只绑 127.0.0.1, 外部网络碰不到, 必须经 adb forward.
static void serve_forever() {
    int lfd = socket(AF_INET, SOCK_STREAM, 0);
    if (lfd < 0) _exit(1);
    fcntl(lfd, F_SETFD, FD_CLOEXEC);  // 别把监听 fd 漏进 helper 子进程
    int opt = 1;
    setsockopt(lfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = htons(APPLIST_PORT);
    if (bind(lfd, (sockaddr *) &addr, sizeof(addr)) != 0) _exit(2);
    if (listen(lfd, 4) != 0) _exit(3);

    for (;;) {
        int cfd = accept(lfd, nullptr, nullptr);
        if (cfd < 0) continue;

        char cmd = 0;
        ssize_t rn = read(cfd, &cmd, 1);
        LOGD("accept cfd=%d read=%zd cmd=%c", cfd, rn, cmd);
        if (rn != 1) { close(cfd); continue; }

        if (cmd == 'Q' || cmd == 'E') {
            // Q = 应用清单(pkg/label/版本);  E = apk 文件清单(含 split, 不传数据)
            char *json = query_helper(cmd == 'E' ? "--manifest" : nullptr);
            if (!json) json = (char *) "{\"error\":\"helper failed\"}";
            size_t len = strlen(json);
            if (len && json[len - 1] == '\n') len--;
            write(cfd, json, len);
            write(cfd, "\n", 1);
        } else if (cmd == 'D') {
            // D = 流式导出. 接下来每行一个包名, 空行结束; 或 "ALL" 一行导出全部.
            const char *want[64];
            int npkg = 0;
            bool all = false;
            char line[PATH_MAX];
            ssize_t llen;
            while ((llen = read_line(cfd, line, sizeof(line))) > 0) {
                if (strcmp(line, "ALL") == 0) { all = true; break; }
                if (npkg < 64) want[npkg++] = strdup(line);
            }
            export_files(cfd, want, npkg, all);
        }
        close(cfd);
    }
}

static void companion_handler(int sock) {
    // 读握手行: "INIT <module_dir>\n" —— 首个请求负责点亮 TCP 服务.
    char line[PATH_MAX + 8];
    ssize_t len = read(sock, line, sizeof(line) - 1);
    LOGD("companion handler: sock=%d len=%zd", sock, len);
    if (len > 0) {
        line[len] = '\0';
        if (strncmp(line, "INIT ", 5) == 0 && !service_started) {
            char *dir = line + 5;
            char *nl = strchr(dir, '\n');
            if (nl) *nl = '\0';
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

// 注册入口 —— 相当于内核模块的 module_init() / kthread_run()
REGISTER_ZYGISK_MODULE(ApplistModule)
REGISTER_ZYGISK_COMPANION(companion_handler)
