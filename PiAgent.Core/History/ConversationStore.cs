using System.Text.Json;
using PiAgent.Core.Agent;

namespace PiAgent.Core.History;

/// <summary>一次可持久化的会话（对话历史 + 元数据），供桌面壳做「历史」列表。</summary>
public sealed record Conversation(
    string Id,
    string Title,
    DateTimeOffset CreatedAt,
    DateTimeOffset UpdatedAt,
    IReadOnlyList<ChatMessage> Messages);

/// <summary>会话历史存储抽象。</summary>
public interface IConversationStore
{
    Task<IReadOnlyList<Conversation>> ListAsync(CancellationToken ct = default);
    Task<Conversation?> GetAsync(string id, CancellationToken ct = default);
    Task SaveAsync(Conversation conversation, CancellationToken ct = default);
    Task DeleteAsync(string id, CancellationToken ct = default);
}

/// <summary>把每个会话存成 <c>{dir}/{id}.json</c> 的本地实现。零依赖，够桌面壳直接用。</summary>
public sealed class JsonConversationStore(string directory) : IConversationStore
{
    private static readonly JsonSerializerOptions Json =
        new(JsonSerializerDefaults.Web) { WriteIndented = true };

    private string PathFor(string id) => Path.Combine(directory, id + ".json");

    public async Task<IReadOnlyList<Conversation>> ListAsync(CancellationToken ct = default)
    {
        if (!Directory.Exists(directory)) return Array.Empty<Conversation>();
        var list = new List<Conversation>();
        foreach (var file in Directory.EnumerateFiles(directory, "*.json"))
        {
            try
            {
                await using var s = File.OpenRead(file);
                var c = await JsonSerializer.DeserializeAsync<Conversation>(s, Json, ct).ConfigureAwait(false);
                if (c is not null) list.Add(c);
            }
            catch (JsonException) { /* 跳过损坏文件 */ }
        }
        return list.OrderByDescending(c => c.UpdatedAt).ToList();
    }

    public async Task<Conversation?> GetAsync(string id, CancellationToken ct = default)
    {
        var path = PathFor(id);
        if (!File.Exists(path)) return null;
        await using var s = File.OpenRead(path);
        return await JsonSerializer.DeserializeAsync<Conversation>(s, Json, ct).ConfigureAwait(false);
    }

    public async Task SaveAsync(Conversation conversation, CancellationToken ct = default)
    {
        Directory.CreateDirectory(directory);
        var tmp = PathFor(conversation.Id) + ".tmp";
        await using (var s = File.Create(tmp))
            await JsonSerializer.SerializeAsync(s, conversation, Json, ct).ConfigureAwait(false);
        File.Move(tmp, PathFor(conversation.Id), overwrite: true); // 原子替换，防半写
    }

    public Task DeleteAsync(string id, CancellationToken ct = default)
    {
        var path = PathFor(id);
        if (File.Exists(path)) File.Delete(path);
        return Task.CompletedTask;
    }
}
