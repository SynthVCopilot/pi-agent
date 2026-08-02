using System.Text.Json.Nodes;
using PiAgent.Core.Agent;
using PiAgent.Core.Components;
using PiAgent.Core.History;
using PiAgent.Core.Mcp;

namespace PiAgent.Core;

/// <summary>Pi Agent 运行时配置。桌面壳的「配置 agent」页面读写它。</summary>
public sealed record PiAgentOptions
{
    /// <summary>synthv-agent-bridge 仓库根目录（含已构建的 dist/src/cli.js）。</summary>
    public string? SynthVBridgeRepoDir { get; init; }

    /// <summary>会话历史存储目录。</summary>
    public string HistoryDirectory { get; init; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PiAgent", "history");

    /// <summary>组件安装根目录。</summary>
    public string ComponentsDirectory { get; init; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PiAgent", "components");

    /// <summary>agent 后端提供者 id（如 "anthropic" / "codex-cli"）。</summary>
    public string ProviderId { get; init; } = "anthropic";

    /// <summary>传给桥子进程的额外环境变量（如自定义 IPC 目录）。</summary>
    public IReadOnlyDictionary<string, string> BridgeEnvironment { get; init; } = new Dictionary<string, string>();
}

/// <summary>
/// pi-agent 的进程内门面（in-proc facade）。这是桌面壳（pi-desktop）唯一需要触碰的类型：
/// 它以纯方法调用/事件的方式在同进程内驱动 agent，无需网络或额外 IPC——满足
/// 「原生 WinUI 3 实现内部通信」的要求。桥子进程 (Node) 与组件子进程 (ffmpeg 等)
/// 由本 host 在内部管理。
/// </summary>
public sealed class PiAgentHost(
    PiAgentOptions options,
    IAgentProvider provider,
    IConversationStore? historyStore = null) : IAsyncDisposable
{
    private SynthVBridge? _bridge;

    /// <summary>会话历史存储。</summary>
    public IConversationStore History { get; } = historyStore ?? new JsonConversationStore(options.HistoryDirectory);

    /// <summary>可安装组件目录（供安装页面渲染）。</summary>
    public IReadOnlyList<ComponentSpec> Components => ComponentCatalog.Default;

    /// <summary>当前配置。</summary>
    public PiAgentOptions Options => options;

    /// <summary>按需连接 SynthV 桥（拉起 node dist/src/cli.js 并握手）。</summary>
    public async Task<SynthVBridge> ConnectBridgeAsync(CancellationToken ct = default)
    {
        if (_bridge is not null) return _bridge;
        if (string.IsNullOrWhiteSpace(options.SynthVBridgeRepoDir))
            throw new InvalidOperationException("未配置 SynthVBridgeRepoDir。请在「配置 agent」里指向 synthv-agent-bridge 仓库根。");
        _bridge = await SynthVBridge.ConnectAsync(options.SynthVBridgeRepoDir!, options.BridgeEnvironment, ct).ConfigureAwait(false);
        return _bridge;
    }

    /// <summary>为一个会话构造 agent 主循环，工具执行器路由到 SynthV 桥。</summary>
    public AgentLoop CreateLoop(IToolExecutor executor) => new(provider, executor);

    /// <summary>把 SynthV 桥的六工具直接暴露为模型可见工具的默认执行器。</summary>
    public async Task<IToolExecutor> CreateBridgeToolExecutorAsync(CancellationToken ct = default)
    {
        var bridge = await ConnectBridgeAsync(ct).ConfigureAwait(false);
        return new BridgeToolExecutor(bridge);
    }

    public async ValueTask DisposeAsync()
    {
        if (_bridge is not null) await _bridge.DisposeAsync().ConfigureAwait(false);
    }
}

/// <summary>把六个 v3 工具原样暴露给模型，并将调用路由到桥。</summary>
internal sealed class BridgeToolExecutor(SynthVBridge bridge) : IToolExecutor
{
    private static readonly string[] ToolNames =
        ["sv_status", "sv_describe", "sv_query", "sv_command", "sv_ui", "sv_review"];

    public IReadOnlyList<ToolDefinition> Tools { get; } = ToolNames
        .Select(n => new ToolDefinition(n, $"SynthV Agent Bridge 工具 {n}（入参 schema 见 sv_describe）。",
            /* lang=json */ "{\"type\":\"object\",\"additionalProperties\":true}"))
        .ToArray();

    public async Task<ToolResult> ExecuteAsync(ToolCall call, CancellationToken ct = default)
    {
        JsonObject args;
        try { args = JsonNode.Parse(call.ArgumentsJson)?.AsObject() ?? new JsonObject(); }
        catch { return new ToolResult(call.Id, "{\"error\":\"参数不是合法 JSON 对象\"}", IsError: true); }

        try
        {
            var result = await bridge.Call(call.ToolName, args, ct).ConfigureAwait(false);
            return new ToolResult(call.Id, result?.ToJsonString() ?? "null");
        }
        catch (McpException ex)
        {
            return new ToolResult(call.Id, $"{{\"error\":{System.Text.Json.JsonSerializer.Serialize(ex.Message)}}}", IsError: true);
        }
    }
}
