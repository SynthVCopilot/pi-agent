using System.Diagnostics;
using System.Collections.Concurrent;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace PiAgent.Core.Mcp;

/// <summary>
/// 极简 MCP (Model Context Protocol) stdio 客户端：以子进程方式拉起一个本地 stdio
/// MCP server（例如 synthv-agent-bridge 的 <c>node dist/src/cli.js</c>），
/// 通过换行分隔的 JSON-RPC 2.0 消息进行 initialize / tools/list / tools/call。
///
/// 刻意不依赖任何第三方 SDK，方便 pi-agent 作为零依赖 submodule 被桌面壳引用。
/// </summary>
public sealed class McpStdioClient : IAsyncDisposable
{
    private readonly Process _process;
    private readonly ConcurrentDictionary<long, TaskCompletionSource<JsonNode?>> _pending = new();
    private readonly SemaphoreSlim _writeLock = new(1, 1);
    private long _nextId;
    private Task? _readLoop;
    private volatile bool _disposed;

    private McpStdioClient(Process process) => _process = process;

    /// <summary>拉起一个 stdio MCP server 子进程并开始读取其输出。</summary>
    public static McpStdioClient Start(McpServerSpec spec)
    {
        var psi = new ProcessStartInfo
        {
            FileName = spec.Command,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardInputEncoding = Encoding.UTF8,
            WorkingDirectory = spec.WorkingDirectory ?? Environment.CurrentDirectory,
        };
        foreach (var a in spec.Arguments) psi.ArgumentList.Add(a);
        foreach (var (k, v) in spec.Environment) psi.Environment[k] = v;

        var process = new Process { StartInfo = psi, EnableRaisingEvents = true };
        if (!process.Start())
            throw new InvalidOperationException($"无法启动 MCP server 进程: {spec.Command}");

        var client = new McpStdioClient(process);
        client._readLoop = Task.Run(client.ReadLoopAsync);
        return client;
    }

    /// <summary>MCP 握手。返回 server 的 initialize 结果。</summary>
    public async Task<JsonNode?> InitializeAsync(string clientName, string clientVersion, CancellationToken ct = default)
    {
        var result = await RequestAsync("initialize", new JsonObject
        {
            ["protocolVersion"] = "2025-06-18",
            ["capabilities"] = new JsonObject { ["tools"] = new JsonObject() },
            ["clientInfo"] = new JsonObject { ["name"] = clientName, ["version"] = clientVersion },
        }, ct).ConfigureAwait(false);
        await NotifyAsync("notifications/initialized", null, ct).ConfigureAwait(false);
        return result;
    }

    /// <summary>列出 server 暴露的工具（synthv-agent-bridge 为六个 v3 工具）。</summary>
    public async Task<JsonArray> ListToolsAsync(CancellationToken ct = default)
    {
        var result = await RequestAsync("tools/list", new JsonObject(), ct).ConfigureAwait(false);
        return result?["tools"]?.AsArray() ?? new JsonArray();
    }

    /// <summary>调用一个工具，返回其 <c>result</c> 节点（含 content / isError）。</summary>
    public Task<JsonNode?> CallToolAsync(string name, JsonObject arguments, CancellationToken ct = default)
        => RequestAsync("tools/call", new JsonObject { ["name"] = name, ["arguments"] = arguments }, ct);

    private async Task<JsonNode?> RequestAsync(string method, JsonNode? @params, CancellationToken ct)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(McpStdioClient));
        var id = Interlocked.Increment(ref _nextId);
        var tcs = new TaskCompletionSource<JsonNode?>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;

        var msg = new JsonObject { ["jsonrpc"] = "2.0", ["id"] = id, ["method"] = method };
        if (@params is not null) msg["params"] = @params;
        await WriteMessageAsync(msg, ct).ConfigureAwait(false);

        using var reg = ct.Register(() => tcs.TrySetCanceled(ct));
        return await tcs.Task.ConfigureAwait(false);
    }

    private Task NotifyAsync(string method, JsonNode? @params, CancellationToken ct)
    {
        var msg = new JsonObject { ["jsonrpc"] = "2.0", ["method"] = method };
        if (@params is not null) msg["params"] = @params;
        return WriteMessageAsync(msg, ct);
    }

    private async Task WriteMessageAsync(JsonObject msg, CancellationToken ct)
    {
        var line = msg.ToJsonString(McpJson.Options);
        await _writeLock.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            await _process.StandardInput.WriteLineAsync(line.AsMemory(), ct).ConfigureAwait(false);
            await _process.StandardInput.FlushAsync(ct).ConfigureAwait(false);
        }
        finally { _writeLock.Release(); }
    }

    private async Task ReadLoopAsync()
    {
        try
        {
            string? line;
            while ((line = await _process.StandardOutput.ReadLineAsync().ConfigureAwait(false)) is not null)
            {
                if (string.IsNullOrWhiteSpace(line)) continue;
                JsonNode? node;
                try { node = JsonNode.Parse(line); }
                catch (JsonException) { continue; } // 非 JSON 行（日志）忽略
                if (node is null) continue;

                var idNode = node["id"];
                if (idNode is not null && long.TryParse(idNode.ToString(), out var id) && _pending.TryRemove(id, out var tcs))
                {
                    if (node["error"] is JsonNode err)
                        tcs.TrySetException(new McpException(err["message"]?.ToString() ?? "MCP error", err));
                    else
                        tcs.TrySetResult(node["result"]);
                }
                // 服务器发起的 notification/request 暂不处理（六工具面只做请求-响应）。
            }
        }
        catch (Exception ex) when (!_disposed)
        {
            foreach (var kv in _pending) kv.Value.TrySetException(ex);
            _pending.Clear();
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (_disposed) return;
        _disposed = true;
        try { if (!_process.HasExited) _process.Kill(entireProcessTree: true); } catch { /* best effort */ }
        if (_readLoop is not null) { try { await _readLoop.ConfigureAwait(false); } catch { } }
        _process.Dispose();
        _writeLock.Dispose();
    }
}

/// <summary>如何拉起一个 MCP server 子进程。</summary>
public sealed record McpServerSpec(
    string Command,
    IReadOnlyList<string> Arguments,
    string? WorkingDirectory = null,
    IReadOnlyDictionary<string, string>? Env = null)
{
    public IReadOnlyDictionary<string, string> Environment => Env ?? new Dictionary<string, string>();
}

/// <summary>MCP server 返回的 JSON-RPC error。</summary>
public sealed class McpException(string message, JsonNode? error) : Exception(message)
{
    public JsonNode? Error { get; } = error;
}

internal static class McpJson
{
    public static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web);
}
