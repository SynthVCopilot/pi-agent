namespace PiAgent.Core.Agent;

/// <summary>一条对话消息的角色。</summary>
public enum ChatRole { System, User, Assistant, Tool }

/// <summary>一条对话消息。<see cref="ToolCalls"/> 与 <see cref="ToolCallId"/> 用于工具往返。</summary>
public sealed record ChatMessage(
    ChatRole Role,
    string Content,
    IReadOnlyList<ToolCall>? ToolCalls = null,
    string? ToolCallId = null,
    DateTimeOffset? Timestamp = null);

/// <summary>模型请求的一次工具调用。</summary>
public sealed record ToolCall(string Id, string ToolName, string ArgumentsJson);

/// <summary>一次工具执行的结果，回喂给模型。</summary>
public sealed record ToolResult(string ToolCallId, string ResultJson, bool IsError = false);

/// <summary>模型可见的工具定义（名字 + 描述 + JSON Schema 入参）。</summary>
public sealed record ToolDefinition(string Name, string Description, string InputSchemaJson);

/// <summary>一次 agent 步进的产物：模型文本 + 它想调用的工具。</summary>
public sealed record AgentStep(string? AssistantText, IReadOnlyList<ToolCall> ToolCalls)
{
    public bool WantsTools => ToolCalls.Count > 0;
}

/// <summary>
/// Agent 后端提供者抽象。默认实现是原生 C# 循环直连 Anthropic Claude API
/// （深度自定义路径）；也可实现一个包装 Codex CLI 子进程的 provider。
/// 桌面壳通过它在进程内驱动 agent，无需额外 IPC。
/// </summary>
public interface IAgentProvider
{
    /// <summary>提供者标识（如 "anthropic"、"codex-cli"）。</summary>
    string Id { get; }

    /// <summary>
    /// 给定完整对话与可用工具，产出下一步（助手文本和/或工具调用）。
    /// 由 <see cref="AgentLoop"/> 负责执行工具、把 <see cref="ToolResult"/> 追加回历史并再次步进。
    /// </summary>
    Task<AgentStep> StepAsync(
        IReadOnlyList<ChatMessage> conversation,
        IReadOnlyList<ToolDefinition> tools,
        CancellationToken ct = default);
}

/// <summary>把模型请求的工具调用真正执行（通常路由到 SynthV 桥或本地组件）。</summary>
public interface IToolExecutor
{
    IReadOnlyList<ToolDefinition> Tools { get; }
    Task<ToolResult> ExecuteAsync(ToolCall call, CancellationToken ct = default);
}
