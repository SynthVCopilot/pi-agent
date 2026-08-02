using System.Text.Json.Nodes;

namespace PiAgent.Core.Mcp;

/// <summary>
/// synthv-agent-bridge 的强类型薄封装：把六个稳定 v3 工具
/// (sv_status / sv_describe / sv_query / sv_command / sv_ui / sv_review)
/// 暴露成方法。艺术判断仍留在 Agent 与 synthv-agent / synthv-tuning 技能里；
/// 这里只做「怎么安全驱动桥」的传输层。
/// </summary>
public sealed class SynthVBridge : IAsyncDisposable
{
    private readonly McpStdioClient _client;

    private SynthVBridge(McpStdioClient client) => _client = client;

    /// <summary>
    /// 按桥仓库自带的 .codex/config.toml 约定拉起桥：<c>node dist/src/cli.js</c>，
    /// 工作目录指向 synthv-agent-bridge 仓库根。
    /// </summary>
    public static async Task<SynthVBridge> ConnectAsync(
        string bridgeRepoDir,
        IReadOnlyDictionary<string, string>? env = null,
        CancellationToken ct = default)
    {
        var spec = new McpServerSpec(
            Command: "node",
            Arguments: new[] { "dist/src/cli.js" },
            WorkingDirectory: bridgeRepoDir,
            Env: env);
        var client = McpStdioClient.Start(spec);
        var bridge = new SynthVBridge(client);
        await client.InitializeAsync("pi-agent", "0.1.0", ct).ConfigureAwait(false);
        return bridge;
    }

    /// <summary>连接、Session、能力、trace、组件构建一致性状态。</summary>
    public Task<JsonNode?> StatusAsync(CancellationToken ct = default)
        => Call("sv_status", new JsonObject(), ct);

    /// <summary>列动作或返回单个动作的紧凑 schema（即时 schema，不塞满上下文）。</summary>
    public Task<JsonNode?> DescribeAsync(JsonObject args, CancellationToken ct = default)
        => Call("sv_describe", args, ct);

    /// <summary>只读投影；用 contextMode:"writeIntent" 才能为随后的 command 铸出写能力 Context。</summary>
    public Task<JsonNode?> QueryAsync(JsonObject args, CancellationToken ct = default)
        => Call("sv_query", args, ct);

    /// <summary>校验过的 edit/delete/clone/import/有界批处理。</summary>
    public Task<JsonNode?> CommandAsync(JsonObject args, CancellationToken ct = default)
        => Call("sv_command", args, ct);

    /// <summary>选区、视口、剪贴板、对话框、吸附、坐标、播放控制。</summary>
    public Task<JsonNode?> UiAsync(JsonObject args, CancellationToken ct = default)
        => Call("sv_ui", args, ct);

    /// <summary>发布/查看可选侧栏预览；用户在 SynthV 里 Apply/Dismiss。</summary>
    public Task<JsonNode?> ReviewAsync(JsonObject args, CancellationToken ct = default)
        => Call("sv_review", args, ct);

    /// <summary>透传任意工具名（供高级/未来工具使用）。</summary>
    public Task<JsonNode?> Call(string tool, JsonObject args, CancellationToken ct = default)
        => _client.CallToolAsync(tool, args, ct);

    public ValueTask DisposeAsync() => _client.DisposeAsync();
}
