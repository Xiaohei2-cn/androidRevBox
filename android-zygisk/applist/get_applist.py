#!/usr/bin/env python3
"""applist 模块的电脑端客户端

用法:
    python3 get_applist.py list                      # 应用清单: pkg/label/版本 -> apps.json
    python3 get_applist.py manifest [包名...]         # apk 文件清单(含 split, 不传文件体)
    python3 get_applist.py export 包名 [包名...]      # 流式导出 base+split 到 apks/<pkg>/
    python3 get_applist.py export --all              # 导出全部应用
    python3 get_applist.py install 包名 [包名...]     # 导出并用 pm install 会话装回(还原)

协议 (TCP 127.0.0.1:11500, 经 adb forward):
    Q                    -> 一行 JSON: 应用清单
    E                    -> 一行 JSON: {pkg: [{name,path,size}...]}
    D\n<pkg>\n...\n\n    -> 文件流: 每文件 "F <size> <pkg> <name>\n" + size 字节, 结束 "DONE\n"
"""
import json
import os
import re
import socket
import subprocess
import sys

PORT = 11500


def connect():
    r = subprocess.run(["adb", "forward", f"tcp:{PORT}", f"tcp:{PORT}"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit("adb forward 失败, 检查: 手机已连接? 模块已安装并重启?")
    s = socket.create_connection(("127.0.0.1", PORT), timeout=15)
    s.settimeout(60)
    return s


def read_line(sock, buf=b""):
    # 从 socket 读一行(\n 结尾, 不返回\n). 返回 (line, 剩余缓冲); EOF 时 line=None
    while b"\n" not in buf:
        c = sock.recv(4096)
        if not c:
            return None, buf
        buf += c
    head, _, rest = buf.partition(b"\n")
    return head, rest


def read_exact(sock, n, buf=b""):
    # 读满 n 字节. 返回 (data, 剩余缓冲); data 短于 n 表示连接提前断开
    while len(buf) < n:
        c = sock.recv(min(65536, n - len(buf)))
        if not c:
            break
        buf += c
    return buf[:n], buf[n:]


def cmd_list():
    s = connect()
    s.sendall(b"Q")
    line, _ = read_line(s)
    s.close()
    apps = json.loads(line)
    if isinstance(apps, dict):
        sys.exit(f"模块错误: {apps}")
    print(f"共 {len(apps)} 个应用")
    for a in apps[:10]:
        print(f"  {a['pkg']:<55} => {a['label']} ({a.get('versionName', '?')})")
    if len(apps) > 10:
        print(f"  ... 其余 {len(apps) - 10} 条省略")
    with open("apps.json", "w", encoding="utf-8") as f:
        json.dump(apps, f, ensure_ascii=False, indent=2)
    print("已保存: apps.json")


def cmd_manifest(pkgs):
    s = connect()
    s.sendall(b"E")
    line, _ = read_line(s)
    s.close()
    man = json.loads(line)
    if isinstance(man, dict) and "error" in man and len(man) == 1:
        sys.exit(f"模块错误: {man['error']}")
    if not pkgs:
        n = sum(len(v) for v in man.values())
        print(f"{len(man)} 个应用, 共 {n} 个 apk 文件 (base {len(man)} + split {n - len(man)})")
        multi = [(p, len(v)) for p, v in man.items() if len(v) > 1]
        multi.sort(key=lambda x: -x[1])
        print("分包最多的前 10 个包:")
        for p, c in multi[:10]:
            print(f"  {c:>2} 个文件  {p}")
        return
    for p in pkgs:
        files = man.get(p)
        if files is None:
            print(f"{p}: 未找到")
            continue
        print(f"{p}:")
        for f in files:
            print(f"  {f['size']:>14,}  {f['name']}")


def cmd_export(pkgs, outdir="apks"):
    s = connect()
    # 命令字节 'D' 后直接紧跟包名行(不要换行隔开):
    # 服务端读掉 1 字节 'D' 后, 每行一个包名, 空行结束
    if pkgs == ["--all"]:
        s.sendall(b"DALL\n\n")
    else:
        s.sendall(b"D" + ("\n".join(pkgs) + "\n\n").encode())

    buf = b""
    total_files = 0
    total_bytes = 0
    while True:
        line, buf = read_line(s, buf)
        if line is None:
            break
        if line == b"DONE":
            break
        if line.startswith(b"ERR"):
            s.close()
            sys.exit(f"服务端错误: {line.decode()}")
        if not line.startswith(b"F "):
            continue
        # "F <size> <pkg> <name>"
        parts = line.decode().split(" ", 3)
        size, pkg, name = int(parts[1]), parts[2], parts[3]
        data, buf = read_exact(s, size, buf)
        if len(data) != size:
            s.close()
            sys.exit(f"传输截断: {pkg}/{name} 期望 {size} 实收 {len(data)}")
        pkg_dir = os.path.join(outdir, pkg)
        os.makedirs(pkg_dir, exist_ok=True)
        with open(os.path.join(pkg_dir, name), "wb") as f:
            f.write(data)
        total_files += 1
        total_bytes += size
        print(f"  ✓ {pkg}/{name} ({size:,} 字节)")
    s.close()
    print(f"完成: {total_files} 个文件, {total_bytes:,} 字节 -> {outdir}/")


def cmd_install(pkgs):
    # 还原演示: base+splits 不是一个文件, 而是一次 pm install 会话
    if not pkgs:
        sys.exit("install 需要包名")
    cmd_export(pkgs)
    for p in pkgs:
        d = os.path.join("apks", p)
        if not os.path.isdir(d):
            print(f"{p}: 无导出文件, 跳过")
            continue
        files = sorted(os.listdir(d))
        print(f"{p}: 装回 {len(files)} 个 apk")
        # 注意: adbd 与 root shell 的 mount namespace 不同, su 建的目录 adb push 不可见;
        # 所以直接推到 adbd 确定可见的 /data/local/tmp, 文件名带包名前缀防碰撞.
        for f in files:
            subprocess.run(["adb", "push", os.path.join(d, f), f"/data/local/tmp/{p}__{f}"],
                           stdout=subprocess.DEVNULL, check=True)
        r = subprocess.run(["adb", "shell", "su", "-c", "pm install-create -r"],
                           capture_output=True, text=True, check=True)
        # 输出形如 "Success: create session [123]", 提取会话号
        m = re.search(r"\[(\d+)\]", r.stdout)
        if not m:
            print(f"  install-create 失败: {r.stdout}{r.stderr}")
            continue
        sid = m.group(1)
        for f in files:
            r = subprocess.run(["adb", "shell", "su", "-c",
                                f"pm install-write {sid} {f} /data/local/tmp/{p}__{f}"],
                               capture_output=True, text=True)
            if "Success" not in r.stdout:
                print(f"  install-write {f} 失败: {r.stdout}{r.stderr}")
        r = subprocess.run(["adb", "shell", "su", "-c", f"pm install-commit {sid}"],
                           capture_output=True, text=True)
        print("  " + (r.stdout or r.stderr or "").strip())
        subprocess.run(["adb", "shell", "su", "-c", f"rm -f /data/local/tmp/{p}__*.apk"])


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return
    op, args = sys.argv[1], sys.argv[2:]
    if op == "list":
        cmd_list()
    elif op == "manifest":
        cmd_manifest(args)
    elif op == "export":
        cmd_export(args or sys.exit("export 需要包名列表或 --all"))
    elif op == "install":
        cmd_install(args)
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
