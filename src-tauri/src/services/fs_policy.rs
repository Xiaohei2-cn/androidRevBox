//! 设备侧文件「增删改」的路径边界（读不受此限制）。
//!
//! 为什么必须有这张表：文件页的增删改默认以 **root (su)** 执行（用户的明确要求），
//! 而 root + 任意路径意味着"这个工具能随手删掉 `/data/data` 或改坏系统分区"。
//! 于是把两条要求合在一句话里：**给足权限，但只给在这两棵树上**。
//!
//! - `/data/local/tmp`：调试工作目录（托管的二进制都在这儿）；
//! - `/sdcard` 与它的真身：外部存储；`/sdcard` 在绝大多数设备上是
//!   `/storage/emulated/0` 的符号链接，两个写法指同一个地方，所以**都放行**——
//!   只认其中一个会造出"界面里看得见、点下去被拒"的新毛病（用户从哪条路径进来都该一致）。
//!
//! 前端 `src/lib/deviceFsPolicy.ts` 有一份同名单，只用来把按钮提前禁掉并说明原因；
//! **真正的强制在这里**（前端不可信，也挡不住别的调用方）。改这张表必须两边一起改。

/// 允许增删改的树（规范化后的路径必须等于它，或落在它下面）
pub const WRITABLE_ROOTS: [&str; 3] = ["/data/local/tmp", "/sdcard", "/storage/emulated/0"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteDenied {
    /// 不是绝对路径：相对路径在不同 shell 的当前目录下意思会变，一律不收
    NotAbsolute,
    /// `..` 跳出了根（`/sdcard/../../data/data/x` 这类伪装）
    EscapesRoot,
    /// 规范化之后落在允许的两棵树之外
    Outside,
}

impl WriteDenied {
    /// 给人看的说法：**每一条都带上"允许哪两处"**。
    ///
    /// 被拒的人下一步要问的永远是"那我能在哪儿改"，只说"这个不行"等于让人猜；
    /// 三种拒绝理由共用同一个尾巴，也免得哪天改字改出三种口径。
    pub fn message(&self, path: &str) -> String {
        let why = match self {
            WriteDenied::NotAbsolute => "要改的路径必须是绝对路径",
            WriteDenied::EscapesRoot => "路径里的 .. 会跳出允许范围",
            WriteDenied::Outside => "这里不在允许的范围内（root 也一样）",
        };
        format!(
            "{why}：{path}；只能在 {} 这两处之内",
            WRITABLE_ROOTS.join(" / ")
        )
    }
}

/// 折叠 `.`/`..`、去掉重复斜杠与结尾斜杠。`..` 越出根（跳出顶层）算伪装，直接拒。
pub fn normalize(path: &str) -> Result<String, WriteDenied> {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                stack.pop().ok_or(WriteDenied::EscapesRoot)?;
            }
            other => stack.push(other),
        }
    }
    Ok(format!("/{}", stack.join("/")))
}

/// 这个路径能不能被改：`Ok` 给出规范化后的路径（真正落到设备上的就是它）。
pub fn check_writable(path: &str) -> Result<String, WriteDenied> {
    if !path.starts_with('/') {
        return Err(WriteDenied::NotAbsolute);
    }
    let clean = normalize(path)?;
    for root in WRITABLE_ROOTS {
        if clean == root || clean.starts_with(&format!("{root}/")) {
            return Ok(clean);
        }
    }
    Err(WriteDenied::Outside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_debug_trees_are_writable_in_any_spelling() {
        for ok in [
            "/data/local/tmp",
            "/data/local/tmp/frida-server",
            "/sdcard",
            "/sdcard/Download/x.apk",
            // 同一地方的两种写法：只认一种会让"看得见却改不动"
            "/storage/emulated/0",
            "/storage/emulated/0/Download/x.apk",
            // 折叠之后仍在这两棵树里
            "/sdcard/Download/../x",
            "/data/local/tmp//a/./b",
        ] {
            assert!(
                check_writable(ok).is_ok(),
                "{ok} 该允许：{:?}",
                check_writable(ok)
            );
        }
    }

    #[test]
    fn everything_else_is_refused_and_says_why() {
        for bad in [
            "/data/data/com.android.providers",
            "/data/local/tmpX/payload", // 前缀像但不是那一棵树
            "/system/bin/sh",
            "/sdcard2/x",
            "/storage/emulated/100/x",
            "relative/thing",
            "",
        ] {
            let error = match check_writable(bad) {
                Err(error) => error,
                Ok(got) => panic!("{bad} 不该被允许，居然规范化成 {got}"),
            };
            let text = error.message(bad);
            assert!(
                text.contains("/data/local/tmp"),
                "{text} 必须说清允许哪两处"
            );
            assert!(text.contains("/sdcard"), "{text} 必须说清允许哪两处");
        }
    }

    #[test]
    fn traversal_out_of_the_root_is_its_own_reason() {
        // 这句与「在允许范围之外」是两种毛病：前者是伪装，后者是普通越界，
        // 混成一句话会让人以为"换个写法就能过"
        assert_eq!(
            check_writable("/sdcard/../../data/data/x"),
            Err(WriteDenied::EscapesRoot)
        );
        assert_eq!(
            check_writable("/../../etc/passwd"),
            Err(WriteDenied::EscapesRoot)
        );
        assert_eq!(
            check_writable("/sdcard/../data/local/tmp/x"),
            Ok("/data/local/tmp/x".to_string()),
            ".. 折叠后确实落在允许的树上，就该放行"
        );
    }

    #[test]
    fn normalize_folds_dots_and_duplicate_slashes() {
        assert_eq!(normalize("/a//b/./c/").as_deref(), Ok("/a/b/c"));
        assert_eq!(normalize("/").as_deref(), Ok("/"));
        assert_eq!(normalize("/a/b/../../c").as_deref(), Ok("/c"));
        // 越顶不是"停在根"，而是这条路径本身不可信
        assert_eq!(normalize("/a/../../b"), Err(WriteDenied::EscapesRoot));
    }
}
