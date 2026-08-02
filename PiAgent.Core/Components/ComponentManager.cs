namespace PiAgent.Core.Components;

/// <summary>可安装组件的类别。</summary>
public enum ComponentKind
{
    /// <summary>ffmpeg：音视频转码/抽取，几乎所有音频前处理的基础。</summary>
    Ffmpeg,
    /// <summary>本地 whisper：离线语音识别，用于把人声转成带时间戳的词。</summary>
    WhisperLocal,
    /// <summary>游戏音高识别模型：从演唱/游戏音频提取音高轮廓。</summary>
    PitchModel,
    /// <summary>Sound→(含词)MIDI：把音频（+可选歌词/词时间轴）转成带音节的 MIDI。</summary>
    SoundToMidi,
}

/// <summary>组件安装状态。</summary>
public enum ComponentState { NotInstalled, Downloading, Installing, Ready, Failed }

/// <summary>一个可安装组件的静态描述（来源、校验、落地路径）。</summary>
public sealed record ComponentSpec(
    string Id,
    ComponentKind Kind,
    string DisplayName,
    string Description,
    string Version,
    string DownloadUrl,
    string? Sha256 = null,
    string? ExecutableRelativePath = null,
    long ApproxSizeBytes = 0);

/// <summary>安装进度回调载荷。</summary>
public sealed record ComponentProgress(string ComponentId, ComponentState State, double Fraction, string? Message = null);

/// <summary>负责下载、校验、解包、登记本地组件。桌面壳的「组件安装」页面调用它。</summary>
public interface IComponentInstaller
{
    /// <summary>目录内已就绪组件的当前状态。</summary>
    Task<ComponentState> GetStateAsync(ComponentSpec spec, CancellationToken ct = default);

    /// <summary>下载 + 校验 SHA-256 + 落地。通过 <paramref name="progress"/> 汇报进度。</summary>
    Task InstallAsync(ComponentSpec spec, IProgress<ComponentProgress>? progress = null, CancellationToken ct = default);

    /// <summary>移除已安装组件。</summary>
    Task UninstallAsync(ComponentSpec spec, CancellationToken ct = default);

    /// <summary>已就绪组件可执行文件的绝对路径（若适用）。</summary>
    string? ResolveExecutable(ComponentSpec spec);
}

/// <summary>
/// 内置组件目录。URL/哈希留作占位，由桌面壳的设置或远端清单填充；
/// 这里给出结构，便于安装页面遍历渲染。
/// </summary>
public static class ComponentCatalog
{
    public static IReadOnlyList<ComponentSpec> Default { get; } = new[]
    {
        new ComponentSpec(
            Id: "ffmpeg",
            Kind: ComponentKind.Ffmpeg,
            DisplayName: "FFmpeg",
            Description: "音视频转码与抽取；whisper / 音高识别 / Sound→MIDI 的前处理基础。",
            Version: "latest",
            DownloadUrl: "", // 由清单填充（如 gyan.dev / BtbN 静态构建）
            ExecutableRelativePath: "bin/ffmpeg.exe"),

        new ComponentSpec(
            Id: "whisper-local",
            Kind: ComponentKind.WhisperLocal,
            DisplayName: "Whisper（本地）",
            Description: "离线语音识别，把人声转成带时间戳的词，喂给 Sound→MIDI 的词轨。",
            Version: "base",
            DownloadUrl: "",
            ExecutableRelativePath: "whisper.exe"),

        new ComponentSpec(
            Id: "pitch-model",
            Kind: ComponentKind.PitchModel,
            DisplayName: "音高识别模型",
            Description: "从演唱/游戏音频提取音高轮廓，供 Sound→MIDI 生成音符音高。",
            Version: "1.0",
            DownloadUrl: ""),

        new ComponentSpec(
            Id: "sound-to-midi",
            Kind: ComponentKind.SoundToMidi,
            DisplayName: "Sound→MIDI（含词）",
            Description: "把音频（+ whisper 词时间轴）转成带音节歌词的 MIDI；也支持直接导入既有 MIDI/MusicXML。",
            Version: "0.1",
            DownloadUrl: ""),
    };
}

/// <summary>
/// Sound→MIDI 管线的高层抽象：结合音高识别与（可选）whisper 词时间轴，
/// 产出「带词 MIDI」；直接导入路径则交给 synthv-agent-bridge 的
/// inspect_score_file / import_monophonic_score。
/// </summary>
public interface ISoundToMidiPipeline
{
    /// <summary>混合模式：音频 → 音高 + 词 → 带词 MIDI 文件，返回落地路径。</summary>
    Task<string> ConvertAsync(SoundToMidiRequest request, IProgress<ComponentProgress>? progress = null, CancellationToken ct = default);
}

/// <summary>Sound→MIDI 请求。</summary>
public sealed record SoundToMidiRequest(
    string AudioPath,
    string OutputMidiPath,
    bool IncludeLyrics = true,
    string? LyricsHint = null,
    string? Language = null);
