namespace PiAgent.Core.Agent;

/// <summary>
/// 与后端无关的 agent 主循环：步进 provider → 执行工具 → 追加结果 → 再步进，
/// 直到模型不再请求工具或达到步数上限。事件用于桌面壳做进程内的实时 UI 更新。
/// </summary>
public sealed class AgentLoop(IAgentProvider provider, IToolExecutor executor, int maxToolIterations = 24)
{
    /// <summary>助手产出文本（含中间步骤）。</summary>
    public event Action<string>? AssistantText;
    /// <summary>模型请求某次工具调用。</summary>
    public event Action<ToolCall>? ToolInvoked;
    /// <summary>某次工具执行完成。</summary>
    public event Action<ToolResult>? ToolCompleted;

    /// <summary>
    /// 跑一整轮：追加用户消息，循环处理工具，返回本轮新增的全部消息（供历史持久化）。
    /// </summary>
    public async Task<IReadOnlyList<ChatMessage>> RunTurnAsync(
        List<ChatMessage> conversation,
        string userInput,
        CancellationToken ct = default)
    {
        var added = new List<ChatMessage>();
        var userMsg = new ChatMessage(ChatRole.User, userInput, Timestamp: DateTimeOffset.UtcNow);
        conversation.Add(userMsg);
        added.Add(userMsg);

        for (var i = 0; i < maxToolIterations; i++)
        {
            ct.ThrowIfCancellationRequested();
            var step = await provider.StepAsync(conversation, executor.Tools, ct).ConfigureAwait(false);

            var assistantMsg = new ChatMessage(
                ChatRole.Assistant,
                step.AssistantText ?? string.Empty,
                ToolCalls: step.WantsTools ? step.ToolCalls : null,
                Timestamp: DateTimeOffset.UtcNow);
            conversation.Add(assistantMsg);
            added.Add(assistantMsg);
            if (!string.IsNullOrEmpty(step.AssistantText)) AssistantText?.Invoke(step.AssistantText);

            if (!step.WantsTools) break;

            foreach (var call in step.ToolCalls)
            {
                ct.ThrowIfCancellationRequested();
                ToolInvoked?.Invoke(call);
                var result = await executor.ExecuteAsync(call, ct).ConfigureAwait(false);
                ToolCompleted?.Invoke(result);
                var toolMsg = new ChatMessage(
                    ChatRole.Tool,
                    result.ResultJson,
                    ToolCallId: result.ToolCallId,
                    Timestamp: DateTimeOffset.UtcNow);
                conversation.Add(toolMsg);
                added.Add(toolMsg);
            }
        }

        return added;
    }
}
