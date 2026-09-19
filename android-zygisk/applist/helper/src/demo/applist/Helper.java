package demo.applist;

// 独立 Java 进程入口, 由 root companion 通过 app_process 启动.
// 支持三种模式 (argv[1]):
//   (无参数)     -> 输出 JSON 数组: 全部应用的 包名/label/版本   (Q 命令)
//   --manifest   -> 输出 JSON 对象: 每包的 apk 文件清单 name/path/size (E 命令, 含 split)
//   --files      -> 输出纯文本路径列表, 每行一个 apk 路径 (D 命令的内部用)
//
// 关键增量: applicationInfo.sourceDir 是 base.apk,
// splitSourceDirs 是全部 split_config.*.apk —— 多分包枚举就靠这两个字段.

import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;

import java.io.BufferedWriter;
import java.io.File;
import java.io.OutputStreamWriter;
import java.lang.reflect.Method;
import java.util.List;

public class Helper {
    public static void main(String[] args) throws Exception {
        BufferedWriter out = new BufferedWriter(
                new OutputStreamWriter(System.out, "UTF-8"));
        String mode = args.length > 0 ? args[0] : "";

        try {
            // systemMain() 内部要创建 Handler, 必须先给主线程准备 Looper
            android.os.Looper.prepareMainLooper();

            Class<?> at = Class.forName("android.app.ActivityThread");
            Method systemMain = at.getMethod("systemMain");
            Object thread = systemMain.invoke(null);
            Method getSystemContext = at.getMethod("getSystemContext");
            Context ctx = (Context) getSystemContext.invoke(thread);

            PackageManager pm = ctx.getPackageManager();
            List<PackageInfo> apps = pm.getInstalledPackages(0);

            StringBuilder sb = new StringBuilder(apps.size() * 128 + 2);

            if ("--files".equals(mode)) {
                // 纯文本: 每行 "包名\t/apk绝对路径". 供 C++ 按包过滤后逐行 open+stream.
                for (PackageInfo pi : apps) {
                    for (String p : apkPaths(pi)) {
                        sb.append(pi.packageName).append('\t').append(p).append('\n');
                    }
                }
                out.write(sb.toString());
            } else if ("--manifest".equals(mode)) {
                // JSON: {"com.xx":[{"name":"base.apk","path":"/data/...","size":123},...], ...}
                sb.append('{');
                boolean firstPkg = true;
                for (PackageInfo pi : apps) {
                    List<String> paths = apkPaths(pi);
                    if (paths.isEmpty()) continue;
                    if (!firstPkg) sb.append(',');
                    firstPkg = false;
                    sb.append(jsonStr(pi.packageName)).append(":[");
                    for (int j = 0; j < paths.size(); j++) {
                        if (j > 0) sb.append(',');
                        String p = paths.get(j);
                        long size = new File(p).length();
                        sb.append("{\"name\":").append(jsonStr(new File(p).getName()))
                          .append(",\"path\":").append(jsonStr(p))
                          .append(",\"size\":").append(size)
                          .append('}');
                    }
                    sb.append(']');
                }
                sb.append('}');
                out.write(sb.toString());
                out.write('\n');
            } else {
                // Q 模式: 应用清单
                sb.append('[');
                for (int i = 0; i < apps.size(); i++) {
                    PackageInfo pi = apps.get(i);
                    ApplicationInfo info = pi.applicationInfo;
                    String label;
                    try {
                        label = pm.getApplicationLabel(info).toString();
                    } catch (Exception e) {
                        label = pi.packageName;
                    }
                    if (i > 0) sb.append(',');
                    sb.append("{\"pkg\":").append(jsonStr(pi.packageName))
                      .append(",\"label\":").append(jsonStr(label))
                      .append(",\"versionName\":").append(jsonStr(pi.versionName != null ? pi.versionName : ""))
                      .append(",\"versionCode\":").append(pi.getLongVersionCode())
                      .append('}');
                }
                sb.append(']');
                out.write(sb.toString());
                out.write('\n');
            }
            out.flush();
        } catch (Throwable t) {
            StringBuilder chain = new StringBuilder();
            for (Throwable c = t; c != null; c = c.getCause()) {
                if (chain.length() > 0) chain.append(" <- ");
                chain.append(c.getClass().getName());
                if (c.getMessage() != null) chain.append(": ").append(c.getMessage());
                StackTraceElement[] st = c.getStackTrace();
                if (st.length > 0) chain.append(" @").append(st[0]);
            }
            out.write("{\"error\":" + jsonStr(chain.toString()) + "}\n");
            out.flush();
            System.exit(1);
        }
        System.exit(0);
    }

    // 一个包对应的全部 APK 文件: base + 所有 split
    private static List<String> apkPaths(PackageInfo pi) {
        List<String> list = new java.util.ArrayList<String>();
        ApplicationInfo ai = pi.applicationInfo;
        if (ai == null) return list;
        if (ai.sourceDir != null) list.add(ai.sourceDir);
        if (ai.splitSourceDirs != null) {
            for (String s : ai.splitSourceDirs)
                if (s != null) list.add(s);
        }
        return list;
    }

    // 手写极简 JSON 字符串转义, 不引入任何第三方依赖
    private static String jsonStr(String s) {
        StringBuilder b = new StringBuilder(s.length() + 2);
        b.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"':  b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n");  break;
                case '\r': b.append("\\r");  break;
                case '\t': b.append("\\t");  break;
                default:
                    if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
                    else b.append(c);
            }
        }
        b.append('"');
        return b.toString();
    }
}
