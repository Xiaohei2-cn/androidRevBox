#!/usr/bin/env python3
"""AR10.5 故障注入用的"假 v2 模块端点"（只在电脑上跑，绝不进产品代码）。

用法（配合 adb reverse，让设备上的 Agent 连到我们）：

    adb -s <serial> shell 'su -c "sh /data/local/tmp/cleanup_11501.sh"'   # 先停真 companion
    adb -s <serial> reverse tcp:11501 tcp:11501
    python3 fake_v2_handshake.py --mode proto3 --seconds 90
    APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools --lib real_agent_zygisk -- --ignored --nocapture
    adb -s <serial> reverse --remove tcp:11501
    adb -s <serial> reboot                                          # 恢复真 companion

每种模式对应一种真实故障（跑法一致：先停真 companion，再 `adb reverse` 到电脑上）：

  proto3     握手回一个**更高的子协议版本**（模块比 Desktop 新）
  garbage    回一行根本不是协议的东西（模块被换坏/端口被别的程序占了）
  close      accept 后立刻断开（半死的 companion、system_server 正在重启）
  slow       收下握手但迟迟不回（companion 卡住，考察 Agent 的超时与降级）
  nocap      回一个合法但**不宣告任何能力**的老握手（旧模块 + 新 Agent）
  errline    握手正常，但命令的**错误**按 v2.1 的老写法发成裸文本行（旧模块现场）
  errframe   握手正常，命令的错误按 v2.2 的约定**装进一帧**里发（新模块现场）

  errline / errframe 是同一个故障的两个版本：模块 helper 失败时，`helper_timeout`
  这句话到底送不送得到调用方嘴里。v2.1 把 ERR 写成裸行，Agent 的帧读取会把那 4 个
  ASCII 字节（`ERR `）当成 1.16 GB 的"帧长度"，界面上只剩一句"响应单帧超过大小上限"
  ——真原因整个丢掉。这两条腿分别钉住"新模块说真话"与"旧模块也不许说假话"。

⚠️ 令牌红线：`H 2 <128hex>` 里的令牌**不打印、不落盘**，日志里只留命令与前缀。
   这是 INTEGRATION.md 第 2 条红线，测试工具同样必须守——否则一次截图就泄露鉴权材料。
"""

import argparse
import socket
import struct
import socketserver
import threading
import time

HELLO_OK = "OK 2 v9.9 99 zh-CN list manifest export describe handlers\n"


class Handler(socketserver.StreamRequestHandler):
    def handle(self) -> None:  # noqa: D102
        mode = self.server.mode  # type: ignore[attr-defined]
        started = time.time()
        try:
            request = self.rfile.readline(256).decode("utf-8", "replace").rstrip("\r\n")
        except OSError:
            request = ""
        # 只记录协议形状，绝不记录令牌
        shape = request.split(" ")[0] if request else "<无>"
        token_hint = "带令牌" if len(request.split(" ")) > 2 else "无令牌参数"
        print(f"[假模块] 收到 {shape} ({token_hint})，模式 {mode}", flush=True)

        if mode == "close":
            print("[假模块] 立刻断开", flush=True)
            return
        if mode == "garbage":
            self.wfile.write(b"hello? what is this\n")
        elif mode == "proto3":
            self.wfile.write(b"OK 3 9.9.9 99 zh-CN list manifest export describe handlers\n")
        elif mode == "nocap":
            self.wfile.write(b"OK 2 1.0 1 zh-CN\n")
        elif mode == "slow":
            time.sleep(max(0.0, self.server.slow) - (time.time() - started))  # type: ignore[attr-defined]
            self.wfile.write(HELLO_OK.encode())
        else:
            self.wfile.write(HELLO_OK.encode())
        self.wfile.flush()

        if mode not in ("errline", "errframe"):
            return
        # 这两种模式要走完一整条命令：握手之后等 Agent 把命令发过来，再回错误。
        # 注册表问答（I）故意保持正常：故障只落在清单那一条方法上，
        # 免得把"探测失败"和"命令失败"混成一件事，看不出到底哪一环在说话。
        while True:
            try:
                command = self.rfile.readline(256).decode("utf-8", "replace").rstrip("\r\n")
            except OSError:
                return
            if not command:
                return
            head = command.split(" ")[0]
            print(f"[假模块] 收到命令 {head}，模式 {mode}", flush=True)
            if head == "I":
                registry = b'{"final":true,"count":0}\n'
                self.wfile.write(struct.pack(">I", len(registry)) + registry)
                self.wfile.flush()
                continue
            body = b"ERR helper_timeout \n"
            if mode == "errframe":
                self.wfile.write(struct.pack(">I", len(body)) + body)
            else:
                self.wfile.write(body)  # v2.1 的老写法：裸行，没有长度前缀
            self.wfile.flush()
            return


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=11501)
    parser.add_argument(
        "--mode",
        default="proto3",
        choices=["proto3", "garbage", "close", "slow", "nocap", "ok", "errline", "errframe"],
    )
    parser.add_argument("--slow", type=float, default=20.0, help="slow 模式的等待秒数")
    parser.add_argument("--seconds", type=float, default=60.0, help="运行多久后自动退出")
    args = parser.parse_args()

    with Server((args.host, args.port), Handler) as server:
        server.mode = args.mode  # type: ignore[attr-defined]
        server.slow = args.slow  # type: ignore[attr-defined]
        print(f"[假模块] 监听 {args.host}:{args.port}，模式 {args.mode}", flush=True)
        stop_at = time.time() + args.seconds
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        while time.time() < stop_at:
            time.sleep(0.3)
        print("[假模块] 到点收工", flush=True)


if __name__ == "__main__":
    main()
