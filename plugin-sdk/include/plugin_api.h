/*
 * plugin_api.h — Android Local Toolbox 插件 C ABI，版本 1（P4 冻结）
 *
 * ⚠️ ABI 冻结纪律（PHASES §7.3）：本文件签名即 v1 契约，
 *    P5–P8 只允许【新增导出符号】，禁止修改/删除既有签名与结构体布局。
 *    任何破坏性变更必须提升 AT_PLUGIN_ABI_VERSION 并同步 host 校验。
 *
 * 导出符号（每个插件动态库必须提供全部五个）：
 *   const AtPluginInfo* at_plugin_info(void);
 *   int   at_plugin_init(const void* host);      // v1 恒传 NULL；返回值 0=成功，非0=错误码
 *   int   at_plugin_call(const uint8_t* input, size_t input_len,
 *                        uint8_t** output, size_t* output_len);
 *   void  at_plugin_free(uint8_t* output, size_t output_len);
 *   void  at_plugin_shutdown(void);
 *
 * 内存所有权（谁分配谁释放）：
 *   - at_plugin_call 的 *output 由【插件】分配（与插件同分配器），
 *     host 读取后必须且只能经【插件自己的 at_plugin_free】释放；
 *     禁止 host 用 free()/delete 释放，禁止插件在 call 返回后自行回收。
 *   - input 归 host 所有，插件只读，不得跨调用持有指针。
 *
 * 线程契约：host 以全局互斥串行化对同一插件的所有调用，
 *          插件实现无需自带锁，但不得依赖多线程并发 call。
 *
 * 载荷协议：v1 payload 为 UTF-8 JSON（由具体插件类型进一步约定字段，
 *          如 crypto 类插件的 AlgorithmDescriptor/execute，P5 定稿）。
 *
 * Rust 插件经 at-plugin-sdk 的 export_plugin! 宏实现本契约，
 * 详见 plugin-sdk/README.md。
 */
#ifndef AT_PLUGIN_API_H
#define AT_PLUGIN_API_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AT_PLUGIN_ABI_VERSION 1

/* 元信息结构体布局即 ABI 一部分，v1 禁止改字段顺序/类型 */
typedef struct {
    uint32_t abi_version; /* 必须 == AT_PLUGIN_ABI_VERSION，否则 host 拒绝加载 */
    const char* id;       /* 全局唯一，形如 "crypto.base64"，[a-z0-9._-] */
    const char* name;     /* 展示名 */
    const char* version;  /* 插件自身语义版本（与 abi 无关） */
    const char* type;     /* device | tool | crypto | parser | workflow */
} AtPluginInfo;

typedef int (*at_plugin_init_fn)(const void* host);
typedef int (*at_plugin_call_fn)(const uint8_t* input, size_t input_len,
                                 uint8_t** output, size_t* output_len);
typedef void (*at_plugin_free_fn)(uint8_t* output, size_t output_len);
typedef void (*at_plugin_shutdown_fn)(void);
typedef const AtPluginInfo* (*at_plugin_info_fn)(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* AT_PLUGIN_API_H */
