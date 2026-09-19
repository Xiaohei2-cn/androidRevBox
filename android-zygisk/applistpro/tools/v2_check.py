#!/usr/bin/env python3
"""ApplistPro v2 真机验收脚本（阶段文档 AR5.6 完成条件对应）

用法:
    python3 tools/v2_check.py            # 需要 adb 已连接、模块已安装并重启生效

覆盖: 鉴权 / 非法输入 / 指定 locale 证据性 / 停用包 / 分帧清单 / 流式导出 /
     no_files / 弃连重连 / forward 清理。任一失败以退出码 1 结束。
"""
import json
import socket
import struct
import subprocess
import sys
import time

PORT = 11501
TOKEN_PATH = "/data/adb/modules/applistpro/token"
SAMPLE_APK = "com.android.internal.display.cutout.emulation.noCutout"

failures = []


def check(name, ok, detail=""):
    print(f"{'PASS' if ok else 'FAIL'}  {name}{('  ' + detail) if detail else ''}")
    if not ok:
        failures.append(name)


def err_code(line):
    """按协议切分 `ERR <code>[ <msg>]`：无消息时末尾会留一个空格，切分后自然消失。"""
    if not line or not line.startswith("ERR "):
        return None
    parts = line[4:].split()
    return parts[0] if parts else None


def adb(*args):
    return subprocess.run(["adb", *args], capture_output=True, text=True)


def read_line(sock):
    buf = b""
    while not buf.endswith(b"\n"):
        chunk = sock.recv(1)
        if not chunk:
            return None
        buf += chunk
    return buf[:-1].decode(errors="replace")


def read_frames(sock, stop_after=None):
    items = []
    while True:
        head = b""
        while len(head) < 4:
            chunk = sock.recv(4 - len(head))
            if not chunk:
                return items, None
            head += chunk
        size = struct.unpack(">I", head)[0]
        body = b""
        while len(body) < size:
            chunk = sock.recv(min(65536, size - len(body)))
            if not chunk:
                break
            body += chunk
        item = json.loads(body.decode())
        items.append(item)
        if item.get("final") or (stop_after and len(items) >= stop_after):
            return items, item


def connect(auth=True, timeout=90):
    sock = socket.create_connection(("127.0.0.1", PORT), timeout=10)
    sock.settimeout(timeout)
    if auth:
        sock.sendall(f"H 2 {token()}\n".encode())
        hello = read_line(sock) or ""
        assert hello.startswith("OK"), hello
    return sock


def token():
    out = adb("shell", "su", "-c", f"cat {TOKEN_PATH}").stdout.strip()
    if len(out) != 128:
        sys.exit(f"读不到 {TOKEN_PATH}（模块未安装/未重启？）: {out!r}")
    return out


def main():
    if adb("devices").stdout.count("\tdevice") == 0:
        sys.exit("没有已连接的 adb 设备")
    adb("forward", f"tcp:{PORT}", f"tcp:{PORT}")
    try:
        # 1 鉴权
        s = socket.create_connection(("127.0.0.1", PORT), timeout=10)
        s.settimeout(10)
        s.sendall(b"H 2 " + b"0" * 128 + b"\n")
        check("错误令牌被拒", err_code(read_line(s)) == "auth_failed")
        s.sendall(b"S\n")
        check("未鉴权命令被拒", err_code(read_line(s)) == "auth_required")
        s.close()

        s = connect()
        for cmd, code in [
            ("E ../escape\n", "ERR bad_package"),
            ("E com.x;reboot\n", "ERR bad_package"),
            ('L $(id) all 0\n', "ERR bad_locale"),
            ("L xx bogus 0\n", "ERR bad_scope"),
            ("NOPE\n", "ERR unknown_command"),
        ]:
            s.sendall(cmd.encode())
            got = read_line(s) or ""
            check(f"非法输入拒绝 {cmd.strip()}", got.startswith(code) and err_code(got) == code.split()[1], got)

        # 2 指定 locale：必须有跨语言证据，且不得回声
        def ask(cmd):
            s.sendall(cmd.encode())
            items, final = read_frames(s)
            return ({i["pkg"]: i for i in items if not i.get("final")}, final or {})

        default, _ = ask("L - all 0\n")
        zh, _ = ask("L zh-CN all 0\n")
        fr, fr_final = ask("L fr-FR all 0\n")
        check("清单非空且带 final 计数", len(default) > 100 and fr_final.get("count", 0) > 100,
              f"{len(default)} 条")
        check("默认请求全部给出可证 locale",
              all(i["resolvedLocale"] for i in default.values()),
              f"null 数={sum(1 for i in default.values() if not i['resolvedLocale'])}")
        proven = [p for p in fr if p in default and fr[p]["label"] != default[p]["label"]]
        echoes = [p for p in fr if fr[p].get("resolvedLocale") == "fr-FR"
                  and fr[p]["label"] == default[p]["label"]]
        unproven = [p for p in fr if fr[p].get("resolvedLocale") is None]
        check("指定 locale 真解析出不同名称", len(proven) > 20, f"{len(proven)} 个包")
        check("resolvedLocale 无回声", not echoes, f"回声={len(echoes)} 无证据置空={len(unproven)}")
        check("中文清单可用", any("\u4e00" <= c <= "\u9fff" for i in zh.values() for c in i["label"]))

        # 3 停用包
        inc, inc_final = ask("L - all 1\n")
        disabled_in_pm = {l.replace("package:", "").strip()
                          for l in adb("shell", "pm", "list", "packages", "-d").stdout.split() if l}
        labeled = {p for p in disabled_in_pm if p in inc and inc[p]["label"] != p}
        check("停用应用带 label 进入清单", len(inc) >= len(default) and len(labeled) >= min(1, len(disabled_in_pm)),
              f"{len(inc)} vs {len(default)}；停用包 {len(disabled_in_pm)} 个，其中 {len(labeled)} 个有 label")

        # 4 manifest + 导出 + no_files
        s.sendall(b"M\n")
        man_items, _ = read_frames(s)
        manifest = {i["pkg"]: i["files"] for i in man_items if not i.get("final")}
        check("分包清单覆盖每个包", bool(manifest) and all(v for v in manifest.values()), f"{len(manifest)} 包")

        if SAMPLE_APK in manifest:
            expect = manifest[SAMPLE_APK][0]
            s.sendall(f"E {SAMPLE_APK}\n".encode())
            line = read_line(s)
            received = None
            while line and line != "DONE":
                if line.startswith("F "):
                    _, size, _, name = line.split(" ", 3)
                    size = int(size)
                    buf = b""
                    while len(buf) < size:
                        chunk = s.recv(min(65536, size - len(buf)))
                        if not chunk:
                            break
                        buf += chunk
                    received = (name, size, len(buf), buf[:2])
                line = read_line(s)
            check("导出大小与 manifest 一致且为 zip",
                  received is not None and received[1] == received[2] == expect["size"]
                  and received[3] == b"PK" and line == "DONE",
                  str(received))
        s.sendall(b"E com.this.package.does.not.exist\n")
        check("不存在包返回 no_files", err_code(read_line(s)) == "no_files")

        # 5 弃连重连
        s.sendall(b"X\n")
        s.close()
        for i in range(3):
            s = connect()
            s.sendall(b"L - all 0\n")
            read_frames(s, stop_after=2)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            s.close()
            started = time.time()
            s2 = connect()
            s2.sendall(b"S\n")
            ok = (read_line(s2) or "").startswith("STATUS")
            s2.sendall(b"X\n")
            s2.close()
            check(f"RST 弃连后重连（第 {i + 1} 轮）", ok and time.time() - started < 5,
                  f"{time.time() - started:.2f}s")
    finally:
        adb("forward", "--remove", f"tcp:{PORT}")
        left = adb("forward", "--list").stdout.strip()
        check("无 forward 残留", f"tcp:{PORT}" not in left, left.splitlines()[-1] if left else "无")

    print("\n失败项:", failures or "无")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
