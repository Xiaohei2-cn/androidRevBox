package pro.applist;

//
// ApplistPro Java helper —— 由 root companion 通过 app_process 冷启动的系统级进程。
// 与 demo 的区别（对应 v2 线协议）：
//   1) 输出改成 NDJSON（每行一个对象 + 末行 final 汇总），C++ 侧按 4 字节长度前缀分帧，
//      不再受「一行 4 MiB」约束；
//   2) --list 支持 locale / scope / include_disabled 三个参数，并给每条结果标注
//      labelSource / resolvedLocale / fallbackReason —— 解析不出来就如实标注，不拿包名冒充；
//   3) --files 直接在 Java 侧按包过滤，避免把全机路径表传给 C++。
//
// 模式（argv[0]）：
//   --list <locale|-> <all|user|system> <0|1>
//   --one <pkg>
//   --manifest
//   --files <pkg>

import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.util.DisplayMetrics;

import java.io.BufferedWriter;
import java.io.File;
import java.io.OutputStreamWriter;
import java.lang.reflect.Method;
import java.util.List;
import java.util.Locale;

public class HelperPro {

    @SuppressWarnings("deprecation")
    public static void main(String[] args) throws Exception {
        BufferedWriter out = new BufferedWriter(new OutputStreamWriter(System.out, "UTF-8"));
        String mode = args.length > 0 ? args[0] : "";

        try {
            // systemMain() 内部要建 Handler，主线程必须先准备 Looper
            android.os.Looper.prepareMainLooper();

            Class<?> at = Class.forName("android.app.ActivityThread");
            Method systemMain = at.getMethod("systemMain");
            Object thread = systemMain.invoke(null);
            Context ctx = (Context) at.getMethod("getSystemContext").invoke(thread);
            PackageManager pm = ctx.getPackageManager();

            if ("--files".equals(mode)) {
                String want = args.length > 1 ? args[1] : "";
                for (PackageInfo pi : pm.getInstalledPackages(0)) {
                    if (!want.isEmpty() && !want.equals(pi.packageName)) continue;
                    ApplicationInfo ai = pi.applicationInfo;
                    if (ai == null) continue;
                    for (String p : apkPaths(ai)) {
                        out.write(pi.packageName);
                        out.write('\t');
                        out.write(p);
                        out.write('\n');
                    }
                }
            } else if ("--one".equals(mode)) {
                // 按包查一次：显示名 + 版本 + 分包集合。
                // 导出与命名只需要一个包，不该为它扫全机（--list 是 O(全机包数) 的
                // getResourcesForApplication，真机上每包一次跨进程资源解析）。
                // 恒用设备默认解析：产物文件名要的就是手机上显示的那个名字。
                String want = args.length > 1 ? args[1] : "";
                PackageInfo one = null;
                try {
                    one = pm.getPackageInfo(want, 0);
                } catch (Throwable ignored) {
                    one = null;
                }
                if (one == null || one.applicationInfo == null) {
                    // 包不存在是调用方的问题：回结构化 notFound，不报 helper_failed，
                    // 否则一个拼错的包名会把这条方法推进熔断。
                    out.write("{\"notFound\":" + jsonStr(want) + "}\n");
                    out.write("{\"final\":true,\"count\":0}\n");
                } else {
                    ApplicationInfo oai = one.applicationInfo;
                    boolean osys = (oai.flags & ApplicationInfo.FLAG_SYSTEM) != 0
                            || (oai.flags & ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0;
                    String odev = currentLocale(ctx);
                    String[] olabel = resolveLabel(pm, oai, "-", odev);
                    List<String> opaths = apkPaths(oai);
                    StringBuilder sb = new StringBuilder(352);
                    sb.append("{\"pkg\":").append(jsonStr(one.packageName))
                      .append(",\"label\":").append(jsonStr(olabel[0]))
                      .append(",\"labelSource\":").append(jsonStr(olabel[1]))
                      .append(",\"requestedLocale\":\"-\"")
                      .append(",\"resolvedLocale\":")
                      .append(olabel[2] == null ? "null" : jsonStr(olabel[2]))
                      .append(",\"fallbackReason\":")
                      .append(olabel[3] == null ? "null" : jsonStr(olabel[3]))
                      .append(",\"versionName\":").append(jsonStr(one.versionName == null ? "" : one.versionName))
                      .append(",\"versionCode\":").append(one.getLongVersionCode())
                      .append(",\"uid\":").append(oai.uid)
                      .append(",\"isSystem\":").append(osys)
                      .append(",\"enabled\":").append(oai.enabled)
                      .append(",\"deviceLocale\":").append(jsonStr(odev))
                      .append(",\"files\":[");
                    for (int j = 0; j < opaths.size(); j++) {
                        if (j > 0) sb.append(',');
                        File f = new File(opaths.get(j));
                        sb.append("{\"name\":").append(jsonStr(f.getName()))
                          .append(",\"size\":").append(f.length()).append('}');
                    }
                    sb.append("]}");
                    out.write(sb.toString());
                    out.write('\n');
                    out.write("{\"final\":true,\"count\":1,\"splitCount\":" + opaths.size() + "}\n");
                }
            } else if ("--manifest".equals(mode)) {
                int count = 0;
                for (PackageInfo pi : pm.getInstalledPackages(0)) {
                    ApplicationInfo ai = pi.applicationInfo;
                    if (ai == null) continue;
                    List<String> paths = apkPaths(ai);
                    if (paths.isEmpty()) continue;
                    StringBuilder sb = new StringBuilder(256);
                    sb.append("{\"pkg\":").append(jsonStr(pi.packageName)).append(",\"files\":[");
                    for (int j = 0; j < paths.size(); j++) {
                        if (j > 0) sb.append(',');
                        String p = paths.get(j);
                        File f = new File(p);
                        sb.append("{\"name\":").append(jsonStr(f.getName()))
                          .append(",\"path\":").append(jsonStr(p))
                          .append(",\"size\":").append(f.length()).append('}');
                    }
                    sb.append("]}");
                    out.write(sb.toString());
                    out.write('\n');
                    count++;
                }
                out.write("{\"final\":true,\"count\":" + count + "}\n");
            } else {
                // --list <locale|-> <scope> <include_disabled>
                String locale = args.length > 1 ? args[1] : "-";
                String scope = args.length > 2 ? args[2] : "all";
                boolean includeDisabled = args.length > 3 && "1".equals(args[3]);

                int flags = 0;
                if (includeDisabled) {
                    flags |= PackageManager.MATCH_DISABLED_COMPONENTS;
                    flags |= PackageManager.MATCH_UNINSTALLED_PACKAGES;
                }
                String deviceLocale = currentLocale(ctx);
                int count = 0;
                int fallback = 0;
                int localeUnproven = 0;

                for (PackageInfo pi : pm.getInstalledPackages(flags)) {
                    ApplicationInfo ai = pi.applicationInfo;
                    if (ai == null) continue;
                    boolean isSystem = (ai.flags & ApplicationInfo.FLAG_SYSTEM) != 0
                            || (ai.flags & ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0;
                    if ("user".equals(scope) && isSystem) continue;
                    if ("system".equals(scope) && !isSystem) continue;
                    boolean enabled = ai.enabled;
                    if (!includeDisabled && !enabled) continue;

                    String[] label = resolveLabel(pm, ai, locale, deviceLocale);
                    if (!"framework".equals(label[1])) fallback++;
                    if ("locale_not_resolved_fallback_default".equals(label[3])
                            || "locale_resources_unavailable".equals(label[3])
                            || "locale_label_missing".equals(label[3])) localeUnproven++;

                    StringBuilder sb = new StringBuilder(224);
                    sb.append("{\"pkg\":").append(jsonStr(pi.packageName))
                      .append(",\"label\":").append(jsonStr(label[0]))
                      .append(",\"labelSource\":").append(jsonStr(label[1]))
                      .append(",\"requestedLocale\":").append(jsonStr(locale))
                      .append(",\"resolvedLocale\":").append(label[2] == null ? "null" : jsonStr(label[2]))
                      .append(",\"fallbackReason\":").append(label[3] == null ? "null" : jsonStr(label[3]))
                      .append(",\"versionName\":").append(jsonStr(pi.versionName == null ? "" : pi.versionName))
                      .append(",\"versionCode\":").append(pi.getLongVersionCode())
                      .append(",\"uid\":").append(ai.uid)
                      .append(",\"isSystem\":").append(isSystem)
                      .append(",\"enabled\":").append(enabled)
                      .append('}');
                    out.write(sb.toString());
                    out.write('\n');
                    count++;
                }

                out.write("{\"final\":true,\"count\":" + count
                        + ",\"fallback\":" + fallback
                        + ",\"localeUnproven\":" + localeUnproven
                        + ",\"deviceLocale\":" + jsonStr(deviceLocale)
                        + ",\"enumeratedFlags\":" + flags + "}\n");
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

    /**
     * 返回 {label, labelSource, resolvedLocale, fallbackReason}。
     *
     * Android 不把 AssetManager 的实际匹配结果导出成公开 API：直接读回填后的
     * Configuration 只会拿到我们塞进去的请求值（真机验证：无 fr 资源的包 label 退回
     * 中文，却仍报 resolved=fr-FR）。所以 resolvedLocale 只有在
     * 「按请求 locale 解析出的 label != 设备默认解析出的 label」时才有证据可报，
     * 否则置 null 并给 locale_not_resolved_fallback_default，不冒充按请求解析成功。
     */
    @SuppressWarnings("deprecation")
    private static String[] resolveLabel(PackageManager pm, ApplicationInfo ai, String locale,
                                         String deviceLocale) {
        String base;
        try {
            base = pm.getApplicationLabel(ai).toString();
        } catch (Throwable t) {
            return new String[]{ai.packageName, "package_name", null, "framework_label_failed"};
        }
        // 「不指定 locale」就是设备默认解析路径，结论本身可证：resolvedLocale = 设备 locale
        if (locale == null || locale.isEmpty() || "-".equals(locale)) {
            return classify(base, ai, "framework", deviceLocale, null);
        }
        boolean sameLanguage = languageOf(locale).equals(languageOf(deviceLocale));

        try {
            Resources app = pm.getResourcesForApplication(ai);
            Configuration want = new Configuration(app.getConfiguration());
            want.setLocale(Locale.forLanguageTag(locale));
            DisplayMetrics dm = app.getDisplayMetrics();
            // 公开构造器顺序是 (assets, metrics, config)，别按 Configuration 在前写
            Resources localized = new Resources(app.getAssets(), dm, want);
            // updateConfiguration 会由 AssetManager 回填「实际命中的配置」，
            // 因此之后读 want 才是真实 resolved locale，而不是照抄请求值。
            localized.updateConfiguration(want, dm);

            CharSequence text = null;
            if (ai.labelRes != 0) {
                try {
                    text = localized.getText(ai.labelRes);
                } catch (Throwable ignored) {
                    text = null;
                }
            }
            if (text == null && ai.nonLocalizedLabel != null) {
                text = ai.nonLocalizedLabel;
            }
            if (text == null) {
                return classify(base, ai, "framework",
                        sameLanguage ? deviceLocale : null, "locale_label_missing");
            }
            String wanted = text.toString();
            boolean differsFromDefault = !wanted.equals(base);
            if (differsFromDefault) {
                return classify(wanted, ai, "framework", want.getLocales().get(0).toLanguageTag(), null);
            }
            // 同名且同语言：走的仍是设备默认解析，据实标为设备 locale，不冒充跨语言命中
            if (sameLanguage) {
                return classify(wanted, ai, "framework", deviceLocale, null);
            }
            return classify(wanted, ai, "framework", null, "locale_not_resolved_fallback_default");
        } catch (Throwable t) {
            // 不能假装按请求 locale 解析成功：如实回退并给出原因
            return classify(base, ai, "framework", null, "locale_resources_unavailable");
        }
    }

    private static String[] classify(String label, ApplicationInfo ai, String source,
                                     String resolved, String reason) {
        if (label == null || label.trim().isEmpty() || label.equals(ai.packageName)) {
            return new String[]{ai.packageName, "package_name", resolved,
                    reason == null ? "label_equals_package_name" : reason};
        }
        return new String[]{label, source, resolved, reason};
    }

    private static String languageOf(String tag) {
        if (tag == null) return "";
        String t = tag.toLowerCase();
        int cut = t.indexOf('-');
        return cut > 0 ? t.substring(0, cut) : t;
    }

    private static String currentLocale(Context ctx) {
        try {
            return ctx.getResources().getConfiguration().getLocales().get(0).toLanguageTag();
        } catch (Throwable t) {
            return "unknown";
        }
    }

    private static List<String> apkPaths(ApplicationInfo ai) {
        List<String> list = new java.util.ArrayList<String>();
        if (ai.sourceDir != null && new File(ai.sourceDir).isFile()) list.add(ai.sourceDir);
        String[] splits = ai.splitSourceDirs;
        if (splits != null) {
            for (String s : splits) {
                if (s != null && new File(s).isFile()) list.add(s);
            }
        }
        return list;
    }

    private static String jsonStr(String s) {
        if (s == null) return "null";
        StringBuilder sb = new StringBuilder(s.length() + 16);
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
            }
        }
        sb.append('"');
        return sb.toString();
    }
}
