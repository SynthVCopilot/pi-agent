# pi-agent (v3)

> SynthVCopilot 的 **v3 深度自定义 agent**，用 **Rust** 写。产物是 C-ABI 原生 DLL，
> 供 WinUI 3 桌面壳 [`pi-desktop`](https://github.com/SynthVCopilot/pi-desktop) 用
> **P/Invoke 进程内直调**（真正的「原生内部通信」，不起 sidecar 子进程）。

## 版本谱系

| 版本 | 是什么 | 归属 |
|---|---|---|
| **v1** | codex skill：`synthv-agent-bridge`（SV2 接入 API）+ `SKILLS/synthv-agent`（运行手册） | 已存在 |
| **v2** | 魔改 codex agent | 不归本项目管 |
| **v3** | **pi**：本仓库，Rust，深度自定义 | 本项目 |

## 工作区

```
pi-agent/  (cargo workspace)
├─ crates/pi-agent-core/   # 与后端无关：agent 循环、会话历史、组件模型（零 async 依赖）
├─ crates/pi-agent-mcp/    # 极简 MCP stdio 客户端 + SynthVBridge（tokio），驱动 synthv-agent-bridge
└─ crates/pi-agent-ffi/    # cdylib「pi_agent.dll」：C-ABI，供 WinUI3 P/Invoke
```

## 组件（AI / 人工均可用）

`pi-agent-core::default_catalog()` 定义，`audience` 标注面向对象（多数为 `Both`）：

| 组件 | 用途 |
|---|---|
| ffmpeg | 转码/抽取，所有音频前处理基础 |
| whisper（本地） | 离线 ASR，人声转带时间戳的词 |
| 游戏音高识别模型 | 提取音高轮廓（AI + 人工） |
| 人声分离（Transformer） | 从混音分出人声/伴奏 stem（Demucs 类） |
| 乐器识别 | 识别混音/stem 的乐器构成 |
| 曲风识别 | 识别风格/流派，辅助选唱法与编曲 |
| 速度与拍点检测 | BPM / beat / downbeat（拍数） |
| Sound→MIDI（含词） | 音频(+词时间轴)→带音节歌词 MIDI；也支持直接导入 |

## 构建

```bash
cargo build              # 全工作区
cargo build -p pi-agent-ffi --release   # 出 target/release/pi_agent.dll
```

需要 Rust 1.92+（msvc）。

## C-ABI（P/Invoke 参考）

```c
char* pi_agent_version(void);
PiAgent* pi_agent_create(void);
void  pi_agent_destroy(PiAgent*);
char* pi_agent_send(PiAgent*, const char* input_utf8); // 返回本轮新增消息 JSON
char* pi_components_json(void);                          // 组件目录 JSON
void  pi_string_free(char*);                             // 释放本库返回的字符串
```

## 状态

骨架：core（agent 循环/历史/组件）+ mcp（stdio 客户端/桥封装）+ ffi（cdylib）三 crate 均 `cargo build` 通过，
默认 `echo` 占位后端。待填充：直连 Claude 的原生 provider、把 mcp 桥接为工具执行器（async→ffi 边界）、
各 ML 组件（人声分离/乐器/曲风/拍点/Sound→MIDI）的实现。

## 许可

Apache-2.0，见 `LICENSE`。
