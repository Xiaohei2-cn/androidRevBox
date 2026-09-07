//! # at-plugin-sdk
//!
//! 用 Rust 实现 host 的 C ABI v1 插件契约（见 `plugin-sdk/include/plugin_api.h`）。
//! 业务开发者只需实现 [`AtPlugin`] trait 并调用 [`export_plugin!`]，
//! 不接触裸指针与生命周期。
//!
//! ## 所有权与内存契约（务必遵守）
//!
//! - `at_plugin_call` 的输出由**本 SDK（即插件侧分配器）分配**；host 读取后
//!   必须且只能回调 `at_plugin_free` 归还。禁止 host 用 `free()`/`delete` 释放。
//! - `input` 归 host 所有，仅在单次调用期间有效，插件不得跨调用持有其指针。
//! - host 以全局锁串行化对同一插件的所有调用，插件实现无需自带锁。
//!
//! ## 错误码约定
//!
//! `call`/`init` 返回 0 表示成功；负值保留给框架：
//! `-99` = 插件 panic 被捕获；`-1` = FFI 参数非法。
//! 正整数错误码由各插件类型自行定义并写进插件 README。
//!
//! ## 示例
//!
//! ```ignore
//! use at_plugin_sdk::{export_plugin, AtPlugin, PluginDescriptor};
//!
//! struct MyPlugin;
//! impl AtPlugin for MyPlugin {
//!     const META: PluginDescriptor = PluginDescriptor {
//!         id: "crypto.demo", name: "Demo", version: "1.0.0", plugin_type: "crypto",
//!     };
//!     fn call(input: &[u8]) -> Result<Vec<u8>, i32> {
//!         Ok(input.to_vec())
//!     }
//! }
//! export_plugin!(MyPlugin);
//! ```

use std::ffi::{CString, c_char, c_int};

/// 当前 ABI 版本（与 plugin_api.h 的 AT_PLUGIN_ABI_VERSION 同步，只增不改）
pub const AT_PLUGIN_ABI_VERSION: u32 = 1;

/// C ABI 的保留错误码
pub const ERR_PANIC: c_int = -99;
pub const ERR_FFI_ARGS: c_int = -1;

/// 与 plugin_api.h 的 AtPluginInfo 逐字段一致的布局（字段名不影响 ABI，顺序/类型才是契约）
#[repr(C)]
#[derive(Debug)]
pub struct AtPluginInfo {
    pub abi_version: u32,
    pub id: *const c_char,
    pub name: *const c_char,
    pub version: *const c_char,
    pub plugin_type: *const c_char,
}

// 指针指向进程生命周期内不可变的 leaked CString，跨线程共享安全
unsafe impl Send for AtPluginInfo {}
unsafe impl Sync for AtPluginInfo {}

/// 插件元信息（以 &'static str 提供，SDK 转为 C 字符串）
pub struct PluginDescriptor {
    /// 全局唯一 id，形如 "crypto.base64"，字符集 [a-z0-9._-]
    pub id: &'static str,
    /// 展示名
    pub name: &'static str,
    /// 插件语义版本
    pub version: &'static str,
    /// device | tool | crypto | parser | workflow
    pub plugin_type: &'static str,
}

/// 插件实现接口：实现本 trait 后用 [`export_plugin!`] 导出五个 C 符号。
pub trait AtPlugin {
    const META: PluginDescriptor;

    /// 加载后、首次 call 前执行；默认无操作
    fn init() -> Result<(), i32> {
        Ok(())
    }

    /// 业务调用入口：入参字节 → 出参字节；错误返回正整数错误码
    fn call(input: &[u8]) -> Result<Vec<u8>, i32>;

    /// 卸载前执行；默认无操作
    fn shutdown() {}
}

/// 转 leaked C 字符串（宏展开会引用 `$crate::into_cstr`，故 pub）
pub fn into_cstr(s: &str) -> *const c_char {
    // 元信息含内部 NUL 属于插件作者 bug，尽早 panic（发生在 info 首次调用，非 FFI 边界）
    CString::new(s)
        .expect("plugin descriptor must not contain NUL")
        .into_raw()
}

/// 导出 C ABI v1 五个符号。在插件 crate 根调用一次：`export_plugin!(MyPlugin);`
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        const _: () = {
            use ::std::ffi::{c_int, c_void};
            use ::std::panic::AssertUnwindSafe;
            use ::std::sync::OnceLock;
            use $crate::{AtPlugin, AtPluginInfo, ERR_FFI_ARGS, ERR_PANIC};

            static __INFO: OnceLock<$crate::AtPluginInfo> = OnceLock::new();

            /// # Safety
            /// 返回指向进程生命周期的不可变结构体，host 只读
            #[unsafe(no_mangle)]
            pub extern "C" fn at_plugin_info() -> *const AtPluginInfo {
                __INFO.get_or_init(|| {
                    let m = <$ty as AtPlugin>::META;
                    AtPluginInfo {
                        abi_version: $crate::AT_PLUGIN_ABI_VERSION,
                        id: $crate::into_cstr(m.id),
                        name: $crate::into_cstr(m.name),
                        version: $crate::into_cstr(m.version),
                        plugin_type: $crate::into_cstr(m.plugin_type),
                    }
                })
            }

            /// # Safety
            /// host 传 NULL 或有效指针；v1 恒传 NULL
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn at_plugin_init(_host: *const c_void) -> c_int {
                match ::std::panic::catch_unwind(AssertUnwindSafe(<$ty as AtPlugin>::init)) {
                    Ok(Ok(())) => 0,
                    Ok(Err(code)) => code,
                    Err(_) => ERR_PANIC,
                }
            }

            /// # Safety
            /// 指针必须来自 host 且长度匹配；输出经 at_plugin_free 归还
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn at_plugin_call(
                input: *const u8,
                input_len: usize,
                output: *mut *mut u8,
                output_len: *mut usize,
            ) -> c_int {
                if output.is_null() || output_len.is_null() {
                    return ERR_FFI_ARGS;
                }
                // 先置空，任何非 0 返回都保证输出无残留
                unsafe {
                    *output = ::std::ptr::null_mut();
                    *output_len = 0;
                }
                if input_len > 0 && input.is_null() {
                    return ERR_FFI_ARGS;
                }
                let bytes: &[u8] = if input_len == 0 {
                    &[]
                } else {
                    unsafe { ::std::slice::from_raw_parts(input, input_len) }
                };
                match ::std::panic::catch_unwind(AssertUnwindSafe(|| {
                    <$ty as AtPlugin>::call(bytes)
                })) {
                    Ok(Ok(data)) => {
                        let boxed: Box<[u8]> = data.into_boxed_slice();
                        let len = boxed.len();
                        let ptr = Box::into_raw(boxed).cast::<u8>();
                        unsafe {
                            *output = ptr;
                            *output_len = len;
                        }
                        0
                    }
                    Ok(Err(code)) => code,
                    Err(_) => ERR_PANIC,
                }
            }

            /// # Safety
            /// ptr/len 必须是 at_plugin_call 成功返回的原值
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn at_plugin_free(output: *mut u8, output_len: usize) {
                if output.is_null() || output_len == 0 {
                    return;
                }
                let slice = unsafe { ::std::slice::from_raw_parts_mut(output, output_len) };
                drop(unsafe { Box::from_raw(slice as *mut [u8]) });
            }

            /// # Safety
            /// FFI 入口；内部 catch_unwind
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn at_plugin_shutdown() {
                let _ = ::std::panic::catch_unwind(AssertUnwindSafe(<$ty as AtPlugin>::shutdown));
            }
        };
    };
}
