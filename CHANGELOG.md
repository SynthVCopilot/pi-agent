# Changelog / 更新日志

## 2026-08-06 — FFmpeg Agent permission boundary / FFmpeg Agent 权限边界

### 中文

#### 权限与产品边界

- Agent 可见的 FFmpeg 工具收紧为两个只读操作：`ffmpeg_probe` 和
  `ffmpeg_loudness_analyze`。
- 移除 Agent 对 `ffmpeg_prepare_audio` 与 `ffmpeg_loudness_normalize` 的定义和执行路由，
  防止对话模型未经用户确认直接生成音频文件。
- 写入能力没有从 Runtime 中删除：`prepare` 与 `loudness_normalize` 继续由
  `pi_ffmpeg_job_start` C ABI 提供给 `pi-desktop`，由 Desktop 显示计划并获得用户确认。
- FFmpeg 组件安装、更新和卸载仍然只能由用户触发；Agent 不获得组件生命周期写权限。

#### 兼容性

- 保持现有六个 FFmpeg/组件 C ABI 导出及 JSON 请求、任务状态和错误结构不变。
- 保留四个有限白名单操作：探测、PCM WAV 准备、EBU R128 响度分析和双遍响度标准化。
- 未增加任意 FFmpeg 参数、远程 URL、系统 PATH 修改或开放式滤镜图入口。

#### 测试与文档

- 新增精确工具面测试，断言 Agent 只能看到两个只读工具。
- 新增拒绝测试，断言两个音频写入工具在执行前以未知工具失败。
- README 和 FFmpeg 组件契约增加 Desktop 确认写入的责任边界说明。
- 验证结果：Rust 工作区 57 项测试通过、2 项按设计忽略；Clippy `-D warnings` 通过；
  本机真实 FFmpeg 四个白名单操作通过。

### English

#### Permission and product boundary

- Reduced the Agent-visible FFmpeg surface to two read-only operations:
  `ffmpeg_probe` and `ffmpeg_loudness_analyze`.
- Removed the Agent definitions and execution routes for `ffmpeg_prepare_audio` and
  `ffmpeg_loudness_normalize`, preventing a conversational model from creating audio files
  without a user-reviewed Desktop action.
- Preserved the write-capable Runtime path: `prepare` and `loudness_normalize` remain available
  through `pi_ffmpeg_job_start` for `pi-desktop`, which presents the plan and asks the user to
  confirm it.
- Component install, update, and uninstall remain user-triggered lifecycle actions; the Agent
  receives no component lifecycle mutation capability.

#### Compatibility

- Kept the existing six FFmpeg/component C ABI exports and their JSON request, job status, and
  structured error contracts unchanged.
- Preserved all four finite allow-listed operations: probe, PCM WAV preparation, EBU R128
  loudness analysis, and two-pass loudness normalization.
- Added no arbitrary FFmpeg arguments, remote URLs, global `PATH` changes, or open-ended filter
  graph execution.

#### Tests and documentation

- Added an exact tool-surface test proving that the Agent sees only the two read-only tools.
- Added rejection coverage proving that both audio-writing tool names fail before execution.
- Updated the README and FFmpeg component contract with the Desktop confirmation boundary.
- Validation: 57 Rust workspace tests passed with 2 intentionally ignored; Clippy passed with
  `-D warnings`; all four allow-listed operations passed against a real local FFmpeg build.
