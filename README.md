# pi-agent

> SynthVCopilot 的**深度自定义 agent 核心**。原生 C# / .NET 10，零第三方依赖，
> 作为 git submodule 被 WinUI 3 桌面壳 [`pi-desktop`](https://github.com/SynthVCopilot/pi-desktop) 以**进程内(in-proc)** 方式直接引用。

## 定位

`pi-agent` 是「大脑 + 编排层」。它：

- 用**原生 C# 的 agent 主循环**（`AgentLoop`）驱动一个可插拔的模型后端（`IAgentProvider`，默认直连 Claude API；也可包装 Codex CLI）。
- 内置一个**极简 MCP stdio 客户端**（`McpStdioClient`），拉起并驱动 [`synthv-agent-bridge`](https://github.com/SynthVCopilot/synthv-agent-bridge) 的六个 v3 工具（`SynthVBridge`）。
- 管理**会话与历史**（`IConversationStore`）。
- 编排**可安装组件**（`IComponentInstaller`：ffmpeg、本地 whisper、音高识别模型、Sound→含词 MIDI）。
- 通过单一门面 `PiAgentHost` 暴露给桌面壳，全部是进程内方法调用/事件，无需额外 IPC。

艺术判断不在这里。「怎么调才好听」在 [`SKILLS/synthv-tuning`](https://github.com/SynthVCopilot/SKILLS)，
「怎么安全驱动桥」在 [`SKILLS/synthv-agent`](https://github.com/SynthVCopilot/SKILLS)。
本仓库只提供把这些技能落地运行的宿主。

## 分层

```
pi-desktop (WinUI 3, C#)  ── 进程内引用 ──▶  PiAgentHost
                                              │
        ┌─────────────────────────────────────┼────────────────────────────┐
        ▼                     ▼                ▼                             ▼
   AgentLoop            SynthVBridge     IConversationStore          IComponentInstaller
 (IAgentProvider)   (McpStdioClient)      (JSON 历史)          (ffmpeg/whisper/pitch/Sound→MIDI)
        │                     │
   Claude API /          node dist/src/cli.js
   Codex CLI             (synthv-agent-bridge, MCP stdio)
```

## 构建

```bash
dotnet build PiAgent.Core/PiAgent.Core.csproj
```

需要 .NET 10 SDK。核心库无第三方 NuGet 依赖。

## 状态

骨架阶段：MCP stdio 客户端、桥封装、agent 循环、历史存储、组件目录与接口已就位；
`IAgentProvider` 的 Anthropic 实现、组件安装器实现、Sound→MIDI 管线为后续填充。

## 许可

见 `LICENSE`。
