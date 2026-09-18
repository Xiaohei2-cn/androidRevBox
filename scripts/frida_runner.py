#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""frida_runner — AppReverseTools P10 Frida 会话工作台的宿主侧 runner。

行协议（NDJSON，单行一个 JSON 对象，字段短名，docs/frida-console-design.md §5.2）：
  {"t":"ready","pid":..,"mode":"spawn"|"attach","pkg":..,"frida":..}  会话建立
  {"t":"log","lvl":"log|info|warn|error","msg":..}                    console.log/warn/error
  {"t":"send","tag":..,"seq":..,"data":..}                            脚本 send() 结构化数据
  {"t":"error","why":..,"stack":..}                                   脚本异常/连接失败
  {"t":"exit","code":..,"why":..}                                     runner 自报退出原因

约定：
- 目标进程退出 → 打 exit 行后以 0 退出；取消由宿主 SIGTERM（task_cancel 链路），
  本脚本注册信号做 detach 清理后退出；
- 只传脚本路径，JS 内容不进命令行；脚本从磁盘读取；
- 逐行 flush（宿主同时传 -u 兜底）；二进制 data（send(payload, ArrayBuffer)）
  按 {"_hex": "..."} 约定并入 data_bin；
- 解析/渲染全部在宿主前端完成，本脚本不感知 UI。

用法（宿主 adapter/frida::build_runner_args 的产出形态）：
  python -u frida_runner.py --usb <serial> | --remote 127.0.0.1:27042
                            (--spawn|--attach) <pkg|pid> --script /abs/path/x.js
"""

import argparse
import json
import os
import signal
import sys
import threading

try:
    import frida
except ImportError:  # 环境缺 frida：协议行 + 非零退出，前端给 error 卡片而非 traceback
    sys.stderr.write("frida 未安装：pip install frida\n")
    sys.exit(3)

_out_lock = threading.Lock()


def emit(obj):
    """按行输出一个协议对象（紧凑 JSON，单行，立即 flush）。"""
    line = json.dumps(obj, ensure_ascii=False, default=str)
    with _out_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def emit_log(lvl, msg):
    emit({"t": "log", "lvl": lvl, "msg": str(msg)})


def emit_error(why, stack=None):
    emit({"t": "error", "why": str(why), "stack": stack})


class Runner:
    def __init__(self, args):
        self.args = args
        self.session = None
        self.script = None
        self.done = threading.Event()
        self.exit_code = 0

    # ===== 设备连接 =====

    def open_device(self):
        if self.args.usb:
            # frida USB device id 与 adb serial 一致（§6.2）
            return frida.get_device(self.args.usb, timeout=15)
        return frida.get_device_manager().add_remote_device(self.args.remote)

    # ===== 会话建立 =====

    def start(self):
        device = self.open_device()
        target = self.args.target
        if self.args.spawn:
            pid = device.spawn([target])
            session = device.attach(pid)
        else:
            attach_target = int(target) if target.isdigit() else target
            session = device.attach(attach_target)
            pid = getattr(session, "pid", None)
        self.session = session
        session.on("detached", self._on_detached)

        script_path = os.path.abspath(self.args.script)
        with open(script_path, "r", encoding="utf-8") as f:
            source = f.read()
        script = session.create_script(source)
        script.on("message", self._on_message)
        script.load()
        self.script = script

        if self.args.spawn:
            device.resume(pid)  # --no-pause 语义：load 完立即恢复

        emit({"t": "ready", "pid": pid,
              "mode": "spawn" if self.args.spawn else "attach",
              "pkg": target, "frida": frida.__version__})

    def _on_detached(self, reason, crash):
        # 目标进程死亡 ≠ 宿主故障：exit 行收尾，前端渲染成卡片（§5.2/§9）
        if not self.done.is_set():
            if crash:
                emit_error(crash.get("summary") or "target crashed", crash.get("report"))
            emit({"t": "exit", "code": 0, "why": str(reason)})
            self.done.set()

    # ===== 消息泵 → NDJSON =====

    def _on_message(self, message, data):
        mtype = message.get("type")
        if mtype == "send":
            payload = message.get("payload")
            tag = seq = None
            if isinstance(payload, dict):
                tag = payload.get("tag")
                seq = payload.get("seq")
            row = {"t": "send", "data": payload}
            if tag is not None:
                row["tag"] = tag
            if seq is not None:
                row["seq"] = seq
            if data:  # send(obj, ArrayBuffer) 的二进制旁路按 _hex 约定并入
                row["data_bin"] = {"_hex": data.hex()}
            emit(row)
        elif mtype == "error":
            emit_error(message.get("description") or "script error", message.get("stack"))
        elif mtype == "stream":
            # console.log/warn/error → frida "stream" 消息，按 §5.2 归入 log
            lvl = message.get("level", "log")
            parts = message.get("data") or []
            emit_log(lvl, " ".join(str(p) for p in parts))
        else:
            # 未知类型不丢：原文降级 log
            emit_log("log", json.dumps(message, ensure_ascii=False, default=str))

    # ===== 生命周期 =====

    def cleanup(self):
        for closer in (self._unload_script, self._detach_session):
            try:
                closer()
            except Exception:
                pass

    def _unload_script(self):
        if self.script:
            self.script.unload()

    def _detach_session(self):
        if self.session:
            self.session.detach()

    def on_signal(self, signum, _frame):
        # 宿主取消（task_cancel → SIGTERM）/Ctrl-C：补 exit 行、清理、退出
        if not self.done.is_set():
            emit({"t": "exit", "code": 0, "why": "signal-%d" % signum})
            self.done.set()
        self.cleanup()
        sys.exit(0)


def parse_args(argv):
    parser = argparse.ArgumentParser(description="AppReverseTools frida runner")
    conn = parser.add_mutually_exclusive_group(required=True)
    conn.add_argument("--usb", help="frida USB device id（= adb serial）")
    conn.add_argument("--remote", help="frida-server host:port（如 127.0.0.1:27042）")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--spawn", metavar="PKG", help="冷启动注入（load 后自动 resume）")
    mode.add_argument("--attach", metavar="PKG_OR_PID", help="附加已运行进程")
    parser.add_argument("--script", required=True, help="JS 脚本绝对路径")
    args = parser.parse_args(argv)
    args.spawn_target = args.spawn
    args.attach_target = args.attach
    args.spawn = bool(args.spawn)
    args.target = args.spawn_target or args.attach_target
    return args


def main(argv=None):
    args = parse_args(argv if argv is not None else sys.argv[1:])
    runner = Runner(args)
    for sig in (signal.SIGINT, signal.SIGTERM) + (
        (signal.SIGHUP,) if hasattr(signal, "SIGHUP") else ()
    ):
        try:
            signal.signal(sig, runner.on_signal)
        except (OSError, ValueError):
            pass

    try:
        runner.start()
    except frida.ProcessNotFoundError as e:
        emit_error("目标进程不存在或未运行: %s" % e)
        emit({"t": "exit", "code": 1})
        return 1
    except frida.TransportError as e:
        emit_error("与 frida-server 通信失败（设备断开/server 未启动/版本不匹配？）: %s" % e)
        emit({"t": "exit", "code": 1})
        return 1
    except Exception as e:  # 连接/注入失败：error 卡片 + exit，不裸 traceback
        emit_error("%s: %s" % (type(e).__name__, e))
        emit({"t": "exit", "code": 1})
        return 1

    try:
        runner.done.wait()  # 目标死亡/detached 或信号唤醒
    except KeyboardInterrupt:
        runner.on_signal(signal.SIGINT, None)
    finally:
        runner.cleanup()
    return runner.exit_code


if __name__ == "__main__":
    sys.exit(main())
