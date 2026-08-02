# -*- coding: utf-8 -*-
"""cvrs — Cross-Version Render Service（跨版本渲染搬运，.svp 文件级）。

背景：SV1(format ≤~134) 与 SV2(≥153) 的 .svp 唱法/参数语义不兼容，
且桥的 Lua API 不能渲染音频/保存工程。CVRS 因此走 **.svp 文件级、只写不读**：
把跨版本的**渲染结果(wav)**当作一条**静音 instrumental 参考轨**写进目标工程，
**绝不读取/翻译异版本工程的可编辑唱法语义**（跨界直译必坏）。

子命令：
  probe <svp>
      只读结构探针：format version → SV1/SV2 时代 → 轨列表（名/音符数/是否
      instrumental/是否静音/音频文件）+ 版本特征标记。不翻译任何唱法数据。

  add-ref <target.svp> --audio <wav> [--name N] [--begin-seconds S] [--out FILE]
      把 wav 作为静音参考音频轨写进目标工程。为保证 schema 与目标版本完全一致，
      从目标自身克隆一个空轨壳（清空所有音符/参数/唱法），只填 isInstrumental+
      audio+mute。输出默认落 ~/.SynthVcopilot/output/ 下（禁 .. 穿透、不覆盖源）。

依赖：标准库即可；wav 时长探测可选用 ffprobe（在 PATH 时自动使用）。
"""
import argparse
import copy
import json
import pathlib
import subprocess
import sys
import uuid

sys.stdout.reconfigure(encoding="utf-8")

# format version → SV 时代边界（见 agents_memory Pi_Agent/002 普查）
SV1_MAX = 134  # 含
SV2_MIN = 153


def data_root() -> pathlib.Path:
    return pathlib.Path.home() / ".SynthVcopilot"


def safe_output_path(name_or_rel: str, subdir: str = "output", suffix: str | None = None) -> pathlib.Path:
    """输出落 ~/.SynthVcopilot/ 数据根；硬禁 '..' 穿透；绝对路径仅根内放行。"""
    root = data_root()
    p = pathlib.PurePath(name_or_rel)
    if any(part == ".." for part in p.parts):
        raise ValueError(f"路径含 '..'，禁止穿透: {name_or_rel}")
    if p.is_absolute():
        resolved = pathlib.Path(name_or_rel).resolve()
        try:
            resolved.relative_to(root.resolve())
        except ValueError:
            raise ValueError(f"绝对路径不在数据根 {root} 内，拒绝: {name_or_rel}")
        out = resolved
    else:
        out = root / subdir / p
    out = pathlib.Path(out)
    if suffix and out.suffix.lower() != suffix:
        out = out.with_suffix(suffix)
    out.parent.mkdir(parents=True, exist_ok=True)
    return out


def load_svp(path: str):
    """容错读 .svp：去 BOM、去尾部 NUL/垃圾（老文件常见）。"""
    raw = pathlib.Path(path).read_bytes()
    if raw[:3] == b"\xef\xbb\xbf":
        raw = raw[3:]
    text = raw.decode("utf-8", errors="replace").rstrip("\x00").strip()
    obj, _end = json.JSONDecoder().raw_decode(text)
    return obj


def era(version) -> str:
    if version is None:
        return "unknown"
    if version <= SV1_MAX:
        return "SV1"
    if version >= SV2_MIN:
        return "SV2"
    return f"boundary({version})"


def ffprobe_duration(wav: str):
    """有 ffprobe 就取时长秒，否则 None。"""
    try:
        out = subprocess.run(
            ["ffprobe", "-v", "quiet", "-show_entries", "format=duration",
             "-of", "default=nk=1:nw=1", wav],
            capture_output=True, text=True, timeout=30,
        )
        return round(float(out.stdout.strip()), 6)
    except Exception:
        return None


def cmd_probe(args) -> dict:
    d = load_svp(args.svp)
    ver = d.get("version")
    tracks = []
    for t in d.get("tracks", []):
        mref = t.get("mainRef") or {}
        mixer = t.get("mixer", {})
        audio = mref.get("audio")
        tracks.append({
            "name": t.get("name"),
            "notes": len((t.get("mainGroup") or {}).get("notes") or []),
            "isInstrumental": bool(mref.get("isInstrumental")),
            "muted": bool(mixer.get("mute") or mref.get("mute")),
            "audioFile": audio.get("filename") if isinstance(audio, dict) else None,
        })
    markers = {
        "group_vocalModes": "vocalModes" in ((d.get("tracks") or [{}])[0].get("mainGroup") or {}),
        "pitchControls": "pitchControls" in ((d.get("tracks") or [{}])[0].get("mainGroup") or {}),
        "startTimeSeconds": "startTimeSeconds" in (d.get("time") or {}),
        "exportPitch": "exportPitch" in (d.get("renderConfig") or {}),
    }
    return {
        "tool": "cvrs/probe",
        "svp": args.svp,
        "version": ver,
        "era": era(ver),
        "trackCount": len(tracks),
        "tracks": tracks,
        "formatMarkers": markers,
        "note": "只读结构探针；不翻译异版本唱法/参数语义（跨界不安全）",
    }


def empty_shell_from(target: dict) -> dict:
    """从目标工程克隆一个轨结构、清空成空 instrumental 壳，保证 schema 与目标版本一致。

    这是'只写不读'纪律下唯一读取目标结构的地方：只搬骨架，清掉全部音符/参数/唱法数据。
    """
    tracks = target.get("tracks") or []
    if not tracks:
        raise ValueError("目标工程没有任何轨，无法克隆 schema 模板")
    shell = copy.deepcopy(tracks[0])
    mg = shell.get("mainGroup") or {}
    mg["notes"] = []
    mg["uuid"] = str(uuid.uuid4())
    # 清空所有参数曲线（保留键与 mode，符合目标版本 schema）
    for pk, pv in (mg.get("parameters") or {}).items():
        if isinstance(pv, dict) and "points" in pv:
            pv["points"] = []
    if "vocalModes" in mg:
        mg["vocalModes"] = {}
    if "pitchControls" in mg:
        mg["pitchControls"] = []
    shell["mainGroup"] = mg
    shell["groups"] = []
    return shell


def cmd_add_ref(args) -> dict:
    d = load_svp(args.target)
    ver = d.get("version")
    shell = empty_shell_from(d)

    duration = ffprobe_duration(args.audio)
    # blicks 是四分音符相对量，秒→blicks 须按速度换算（首个 tempo 常速近似；begin=0 时恒为 0）
    tempo_map = (d.get("time") or {}).get("tempo")
    bpm = 120.0
    if isinstance(tempo_map, list) and tempo_map:
        bpm = float(tempo_map[0].get("bpm", 120.0)) or 120.0
    begin_blicks = int(round(args.begin_seconds * (bpm / 60.0) * 705600000))

    mref = shell.get("mainRef") or {}
    mref["groupID"] = shell["mainGroup"]["uuid"]
    mref["isInstrumental"] = True
    mref["blickAbsoluteBegin"] = begin_blicks
    mref["blickAbsoluteEnd"] = -1
    mref["blickOffset"] = begin_blicks
    mref["pitchOffset"] = 0
    if "mute" in mref:  # v187+
        mref["mute"] = True
    audio_obj = {"filename": args.audio}
    if duration is not None:
        audio_obj["duration"] = duration
    mref["audio"] = audio_obj
    # 删掉 vocal 专属字段（这是参考音频轨，不承载唱法）
    for k in ("takes", "pitchTakes", "timbreTakes", "voice", "voicePresetName",
              "vocalModeParams", "vocalModeInherited", "vocalModePreset"):
        mref.pop(k, None)
    shell["mainRef"] = mref

    shell["name"] = args.name or (pathlib.Path(args.audio).stem + " (CVRS ref)")
    shell["renderEnabled"] = False
    mixer = shell.get("mixer") or {}
    mixer["mute"] = True          # 静音：既 mute 又不参与渲染
    mixer["solo"] = False
    shell["mixer"] = mixer
    shell["dispOrder"] = len(d.get("tracks") or [])

    d.setdefault("tracks", []).append(shell)

    out_name = args.out or (pathlib.Path(args.target).stem + "_cvrs.svp")
    out_path = safe_output_path(out_name, subdir="output", suffix=".svp")
    # SV 写单行 JSON、无 BOM；这里 ensure_ascii=False 保中文，separators 紧凑
    out_path.write_text(
        json.dumps(d, ensure_ascii=False, separators=(",", ":")),
        encoding="utf-8",
    )
    return {
        "tool": "cvrs/add-ref",
        "target": args.target,
        "target_version": ver,
        "target_era": era(ver),
        "added_track": shell["name"],
        "audio": args.audio,
        "audio_duration_sec": duration,
        "muted": True,
        "renderEnabled": False,
        "out": str(out_path),
        "note": "只写：静音参考音频轨已追加；源工程唱法语义未被读取/翻译。源文件未改动。",
    }


def main():
    ap = argparse.ArgumentParser(prog="cvrs", description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("probe", help="只读结构探针：版本/时代/轨列表")
    p.add_argument("svp")
    p.set_defaults(fn=cmd_probe)

    a = sub.add_parser("add-ref", help="把 wav 作为静音参考音频轨写进目标工程")
    a.add_argument("target", help="目标 .svp（写入方；不会被覆盖）")
    a.add_argument("--audio", required=True, help="参考 wav 路径（SV 里相对/绝对均可）")
    a.add_argument("--name", help="新轨名")
    a.add_argument("--begin-seconds", type=float, default=0.0, help="音频起始位置（秒）")
    a.add_argument("--out", help="输出文件名（落数据根 output/；默认 <目标>_cvrs.svp）")
    a.set_defaults(fn=cmd_add_ref)

    args = ap.parse_args()
    try:
        print(json.dumps(args.fn(args), ensure_ascii=False, indent=1))
    except Exception as e:
        print(json.dumps({"tool": "cvrs", "error": f"{type(e).__name__}: {e}"}, ensure_ascii=False))
        sys.exit(1)


if __name__ == "__main__":
    main()
