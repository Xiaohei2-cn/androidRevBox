#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""frida_runner 的测试：目标解析 + 行协议。不起会话、不连设备、不需要装 frida。

为什么单独测它：runner 是宿主与 frida 之间唯一的翻译层，以前一条断言都没有。
用户报「attach 提示目标不能为空」时，我没法证明 `-F`（附加前台）这条语义到底在不在，
只能靠读代码——而这次读代码读出来的结论是错的。

跑法：python3 scripts/test_frida_runner.py（也挂在 m1_regression.sh 上）
"""
import contextlib
import io
import json
import os
import sys
import tempfile
import types

# frida 是大二进制扩展，测试不需要真装上：先塞一个假模块再 import runner，
# 没装 frida 的机器（CI）也能跑同一套断言。
fake = types.ModuleType("frida")
fake.__version__ = "0.0-test"
fake.TransportError = type("TransportError", (Exception,), {})
fake.ProcessNotFoundError = type("ProcessNotFoundError", (Exception,), {})
fake.InvalidArgumentError = type("InvalidArgumentError", (Exception,), {})


class _Script:
    def on(self, *_a, **_k):
        pass

    def load(self):
        pass

    def unload(self):
        pass


class _Session:
    pid = 4321

    def on(self, *_a, **_k):
        pass

    def create_script(self, *_a, **_k):
        return _Script()

    def detach(self):
        pass


class _App:
    def __init__(self, pid, name, identifier):
        self.pid = pid
        self.name = name
        self.identifier = identifier


class _Device:
    """记录调用轨迹：「按 pid 附加」这种断言只能靠它证明。"""

    def __init__(self, frontmost):
        self._frontmost = frontmost
        self.attached = []
        self.spawned = []

    def get_frontmost_application(self, *_a, **_k):
        return self._frontmost

    def attach(self, target, *_a, **_k):
        self.attached.append(target)
        return _Session()

    def spawn(self, argv, *_a, **_k):
        self.spawned.append(tuple(argv))
        return 999

    def resume(self, _pid):
        pass


DEV = types.SimpleNamespace(frontmost=None, last=None)


def _new_device(**_k):
    DEV.last = _Device(DEV.frontmost)
    return DEV.last


fake.get_device = lambda *_a, **_k: _new_device()
fake.get_device_manager = lambda *a, **k: types.SimpleNamespace(
    add_remote_device=lambda *_a, **_k: _new_device())
sys.modules["frida"] = fake

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import frida_runner as R  # noqa: E402


def _start(argv):
    """跑一遍 start()，返回 (协议行, 设备轨迹)。--script 用真临时文件（runner 会读盘）。"""
    with tempfile.TemporaryDirectory() as d:
        script = os.path.join(d, "01hook_tcp.js")
        with open(script, "w", encoding="utf-8") as f:
            f.write("console.log('x');")
        args = R.parse_args(argv + ["--script", script])
        DEV.frontmost = getattr(args, "_frontmost_fixture", None)
        buf = io.StringIO()
        err = None
        with contextlib.redirect_stdout(buf):
            try:
                R.Runner(args).start()
            except Exception as e:  # noqa: BLE001 - 断言的就是抛不抛
                err = e
        lines = [json.loads(x) for x in buf.getvalue().splitlines() if x.strip()]
        return lines, DEV.last, err


def _main(argv):
    """跑 main()（含它的异常兜底），返回 (exit code, 协议行)。"""
    # 每条用例自己决定设备上有没有前台：不留上一条的状态。
    # （踩过：留着上一条的 _App，这条的 start() 直接成功，main() 落进 done.wait() 挂死）
    DEV.frontmost = None
    with tempfile.TemporaryDirectory() as d:
        script = os.path.join(d, "x.js")
        with open(script, "w", encoding="utf-8") as f:
            f.write("// x")
        buf = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(err):
            code = R.main(argv + ["--script", script])
        return code, [json.loads(x) for x in buf.getvalue().splitlines() if x.strip()]


USB = ["--usb", "PIXEL-6"]


def test_01_attach_with_no_target_is_not_an_error():
    """前台模式（frida -F）：不给目标不算漏填，而是"附加当前前台"。"""
    args = R.parse_args(USB + ["--frontmost", "--script", "/w/a.js"])
    assert args.frontmost is True
    assert args.target == "", "前台模式不该带目标串"


def test_02_frontmost_attaches_by_pid_and_reports_the_package():
    DEV.frontmost = _App(pid=4321, name="亚马逊购物",
                         identifier="com.amazon.mShop.android.shopping")
    with tempfile.TemporaryDirectory() as d:
        script = os.path.join(d, "01hook_tcp.js")
        open(script, "w").write("// x")
        args = R.parse_args(USB + ["--frontmost", "--script", script])
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            R.Runner(args).start()
        lines = [json.loads(x) for x in buf.getvalue().splitlines() if x.strip()]
    dev = DEV.last
    # 按 pid 而不是按包名：一只前台 App 常带 :remote/:push 等同名子进程，包名会撞歧义
    assert dev.attached == [4321], dev.attached
    ready = [x for x in lines if x.get("t") == "ready"]
    assert ready, lines
    assert ready[0]["pkg"] == "com.amazon.mShop.android.shopping", ready[0]
    assert ready[0]["pid"] == 4321, ready[0]
    assert ready[0]["mode"] == "attach", "前台模式协议里仍是 attach，不发明新模式"
    logs = " ".join(x.get("msg", "") for x in lines if x.get("t") == "log")
    assert "自动附加当前前台" in logs, "界面上要看得见究竟附上了谁：" + logs


def test_03_no_frontmost_app_says_what_to_do_not_a_python_class_name():
    code, lines = _main(USB + ["--frontmost"])
    assert code == 1, "拿不到前台必须非零退出，不能假装成功"
    errs = [x for x in lines if x.get("t") == "error"]
    assert errs, lines
    why = errs[0]["why"]
    assert "前台应用" in why and "解锁" in why, why
    assert "Error" not in why, "不能把 Python 类名甩给用户：" + why
    assert [x for x in lines if x.get("t") == "exit"], "error 之后还要有 exit 行收尾"


def test_04_attach_still_accepts_package_and_pid():
    for target, expect in (
        ("com.amazon.mShop.android.shopping", "com.amazon.mShop.android.shopping"),
        ("1234", 1234),
    ):
        DEV.frontmost = None
        lines, dev, err = _start(USB + ["--attach", target])
        assert err is None, err
        assert dev.attached == [expect], (target, dev.attached)


def test_05_spawn_still_resumes_and_reports():
    DEV.frontmost = None
    lines, dev, err = _start(USB + ["--spawn", "com.x.y"])
    assert err is None, err
    assert dev.spawned == [("com.x.y",)], dev.spawned
    assert dev.attached == [999], dev.attached
    ready = [x for x in lines if x.get("t") == "ready"][0]
    assert ready["mode"] == "spawn" and ready["pkg"] == "com.x.y", ready


def test_06_bad_invocations_are_rejected_by_argparse():
    def expect_exit(argv):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            try:
                R.parse_args(argv + ["--script", "/w/a.js"])
            except SystemExit as e:
                assert e.code != 0, "argparse 必须非零退出"
                return
        raise AssertionError("应当被 argparse 拒绝：%s" % argv)

    expect_exit(USB + ["--frontmost", "--spawn", "com.x"])  # 模式互斥
    expect_exit(USB + ["--attach", ""])                     # 空目标不许悄悄当前台
    expect_exit(USB)                                        # 一个目标都没给


def _timeout(signum, frame):  # noqa: ARG001
    # 用例挂死（例如误进了 done.wait()）必须响，不要在这里静默卡住整条回归
    raise SystemExit("✗ frida_runner 测试超过 30s 未完成，判定失败（用例挂死了）")


def _run():
    import signal
    signal.signal(signal.SIGALRM, _timeout)
    signal.alarm(30)
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    failed = 0
    for t in tests:
        try:
            DEV.frontmost = None
            t()
            print("  ✓ %s" % t.__name__, flush=True)
        except Exception as e:  # noqa: BLE001
            failed += 1
            print("  ✗ %s -> %s: %s" % (t.__name__, type(e).__name__, e), flush=True)
    print("frida_runner 测试：%d/%d 通过" % (len(tests) - failed, len(tests)), flush=True)
    signal.alarm(0)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(_run())
