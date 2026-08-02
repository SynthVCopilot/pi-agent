# -*- coding: utf-8 -*-
"""pi-audio — Pi Agent 的音频探针组件（AI/人工均可用）。

子命令：
  probe <audio> [--panns] [--notes]
      浅层特征指纹（BPM/调/打击比/能量弧/音区分布）+ 可选 PANNs 判别
      （乐器构成、genre 倾向、有词/无词判别）。输出紧凑 JSON（stdout）。
      风格命名刻意留给上层 LLM：本工具只出结构化事实，不下审美结论。

  pair-diff <vocal> <inst> [--midi OUT.mid] [--tol 0.08]
      有词/无词配对差分：按 (pitch, start±tol) 消耗式匹配去除伴奏音符，
      残差=人声贡献；经"最高音抢占"单音化后可直接喂
      synthv-agent-bridge 的 import_monophonic_score（≤512 音符时）。

依赖：librosa / numpy / basic-pitch / pretty-midi；PANNs 判别需
torch(CPU 即可) + panns-inference。Python ≤3.11（basic-pitch 生态限制）。
"""
import argparse
import json
import sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")

NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# AudioSet 中的风格/情绪类标签（供相对排序；绝对概率普遍偏低，不可单独当结论）
GENREISH = {
    "Pop music", "Rock music", "Electronic music", "Classical music", "Jazz",
    "Hip hop music", "Soundtrack music", "Video game music", "Dance music",
    "Techno", "House music", "Trance music", "Ambient music", "New-age music",
    "Folk music", "Country", "Rhythm and blues", "Soul music", "Funk",
    "Heavy metal", "Punk rock", "Disco", "Electronica", "Electronic dance music",
    "Drum and bass", "Dubstep", "Progressive rock", "Music of Asia",
    "Traditional music", "Opera", "Swing music", "Blues", "Theme music",
    "Happy music", "Sad music", "Tender music", "Exciting music", "Angry music",
    "Scary music", "Music for children", "Lullaby",
}
INSTRUMENTISH = {
    "Piano", "Electric piano", "Acoustic guitar", "Electric guitar", "Bass guitar",
    "Drum kit", "Drum machine", "Synthesizer", "Violin, fiddle", "Cello",
    "Orchestra", "String section", "Brass instrument", "Trumpet", "Saxophone",
    "Flute", "Organ", "Harp", "Bell", "Marimba, xylophone", "Glockenspiel",
    "Choir", "Keyboard (musical)",
}
VOCALISH = {"Speech", "Singing", "Female singing", "Male singing", "Child singing", "Choir", "A capella"}


def note_name(p: int) -> str:
    return f"{NOTE_NAMES[p % 12]}{p // 12 - 1}"


def _ensure_panns_assets():
    """预置 PANNs 所需文件（Cnn14 checkpoint ~300MB + AudioSet 标签 CSV）。

    panns_inference 自带的下载走 os.system('wget')，在无 wget 的 Windows 上
    静默失败；这里用标准库 urllib 补齐，已存在则跳过。
    """
    import os
    import urllib.request

    panns_dir = os.path.join(os.path.expanduser("~"), "panns_data")
    os.makedirs(panns_dir, exist_ok=True)
    assets = [
        (
            os.path.join(panns_dir, "class_labels_indices.csv"),
            "https://raw.githubusercontent.com/qiuqiangkong/audioset_tagging_cnn/master/metadata/class_labels_indices.csv",
        ),
        (
            os.path.join(panns_dir, "Cnn14_mAP=0.431.pth"),
            "https://zenodo.org/record/3987831/files/Cnn14_mAP%3D0.431.pth?download=1",
        ),
    ]
    for dest, url in assets:
        if os.path.exists(dest) and os.path.getsize(dest) > 0:
            continue
        print(f"downloading {os.path.basename(dest)} ...", file=sys.stderr)
        tmp = dest + ".part"
        urllib.request.urlretrieve(url, tmp)
        os.replace(tmp, dest)


def extract_notes(path: str):
    """basic-pitch 音符提取 → [{pitch,start,end,velocity}]，按 start 排序。"""
    import contextlib

    from basic_pitch.inference import predict

    # basic-pitch 会往 stdout 打印进度行，污染本工具的纯 JSON 输出——改道 stderr。
    with contextlib.redirect_stdout(sys.stderr):
        _, _, note_events = predict(path)
    notes = [
        {
            "pitch": int(p),
            "start": round(float(s), 3),
            "end": round(float(e), 3),
            "velocity": int(a * 127),
        }
        for (s, e, p, a, _bends) in note_events
    ]
    notes.sort(key=lambda n: n["start"])
    return notes


def cmd_probe(args) -> dict:
    import librosa

    y, sr = librosa.load(args.audio, sr=22050, mono=True)
    duration = len(y) / sr

    tempo, _ = librosa.beat.beat_track(y=y, sr=sr)
    tempo = float(np.atleast_1d(tempo)[0])

    harmonic, percussive = librosa.effects.hpss(y)
    h_rms = float(np.sqrt(np.mean(harmonic**2)))
    p_rms = float(np.sqrt(np.mean(percussive**2)))
    perc_ratio = p_rms / (h_rms + p_rms) if (h_rms + p_rms) > 0 else 0.0

    chroma = librosa.feature.chroma_cqt(y=y, sr=sr).mean(axis=1)
    key = NOTE_NAMES[int(np.argmax(chroma))]

    # 六段能量弧（0-9 归一化）；极短音频段可能为空 → 回退 0.0，避免 NaN→int 崩溃
    rms = librosa.feature.rms(y=y)[0]
    seg = np.array_split(rms, 6)
    seg_e = [float(np.mean(s)) if s.size else 0.0 for s in seg]
    mx = max(seg_e) or 1.0
    energy_arc = "".join(str(min(9, int(e / mx * 9.99))) for e in seg_e)

    centroid = librosa.feature.spectral_centroid(y=y, sr=sr)[0]
    half = len(centroid) // 2
    trend = float(np.mean(centroid[half:]) - np.mean(centroid[:half]))
    brightness = "rising" if trend > 150 else ("falling" if trend < -150 else "flat")

    result = {
        "tool": "pi-audio/probe",
        "audio": args.audio,
        "duration_sec": round(duration, 1),
        "bpm": round(tempo),
        "bpm_note": "beat-tracking 存在 2x/0.5x 歧义；有配对版本时以一致者为准",
        "key_guess": key,
        "percussive_ratio": round(perc_ratio, 3),
        "energy_arc_6seg": energy_arc,
        "brightness_trend": brightness,
    }

    if args.notes:
        notes = extract_notes(args.audio)
        pitches = [n["pitch"] for n in notes]
        octs: dict[int, int] = {}
        for p in pitches:
            octs[p // 12 - 1] = octs.get(p // 12 - 1, 0) + 1
        long_n = sum(1 for n in notes if n["end"] - n["start"] > 0.8)
        result["notes"] = {
            "total": len(notes),
            "per_minute": round(len(notes) / (duration / 60)) if duration else 0,
            "range": f"{note_name(min(pitches))}-{note_name(max(pitches))}" if pitches else None,
            "long_over_800ms": long_n,
            "octave_histogram": {f"O{o}": c for o, c in sorted(octs.items())},
        }

    if args.panns:
        import contextlib

        # panns_inference 的 import/构造会向 stdout 打印（Checkpoint path/Using CPU），
        # 且首次下载走 os.system('wget')（Windows 无 wget 会静默失败）——
        # 先用 urllib 预置资产，再整体改道 stderr 保证本工具 stdout 纯 JSON。
        _ensure_panns_assets()
        with contextlib.redirect_stdout(sys.stderr):
            from panns_inference import AudioTagging
            from panns_inference.config import labels

            y32, _ = librosa.load(args.audio, sr=32000, mono=True)
            at = AudioTagging(checkpoint_path=None, device="cpu")
            clipwise, _ = at.inference(y32[None, :])
        probs = clipwise[0]
        order = np.argsort(probs)[::-1]

        def pick(pool, k):
            return [
                {"label": labels[i], "p": round(float(probs[i]), 3)}
                for i in order
                if labels[i] in pool
            ][:k]

        vocal_p = float(sum(probs[i] for i, l in enumerate(labels) if l in VOCALISH))
        result["panns"] = {
            "instruments": pick(INSTRUMENTISH, 6),
            "genre_hints": pick(GENREISH, 6),
            "genre_note": "AudioSet genre 概率普遍偏低且对 VOCALOID 音色有儿歌偏置，仅供相对排序；风格命名交给上层 LLM 结合本 JSON 判断",
            "vocal_prob_sum": round(vocal_p, 3),
            # 实测样本分布（12 对中V样本，2026-08）：有词 ≥0.35，无词 ≤0.05。
            # 判决边界有意放宽留余量：≥0.2 判 vocal，≤0.08 判 instrumental，其余 uncertain。
            "has_vocals_verdict": "vocal" if vocal_p >= 0.2 else ("instrumental" if vocal_p <= 0.08 else "uncertain"),
        }

    return result


def diff_notes(vnotes, inotes, tol):
    """人声版音符中去除能在 INST 里按 (pitch, start±tol) 匹配到的（一对一消耗）。"""
    import bisect

    by_pitch: dict[int, list[float]] = {}
    for n in inotes:
        by_pitch.setdefault(n["pitch"], []).append(n["start"])
    for v in by_pitch.values():
        v.sort()
    residual, matched = [], 0
    for n in vnotes:
        starts = by_pitch.get(n["pitch"], [])
        i = bisect.bisect_left(starts, n["start"] - tol)
        if i < len(starts) and abs(starts[i] - n["start"]) <= tol:
            matched += 1
            starts.pop(i)
        else:
            residual.append(n)
    return residual, matched


def mono_collapse(notes):
    """扫描线单音化：任意时刻只留最高音；低音丢弃、前音截断；清除 <60ms 碎屑。

    注意：低于主线的和声声部会被丢弃——提取和声需按音高聚类分层，另行处理。
    """
    ns = sorted((dict(n) for n in notes), key=lambda n: (n["start"], -n["pitch"]))
    out = []
    for n in ns:
        if not out:
            out.append(n)
            continue
        last = out[-1]
        if n["start"] >= last["end"] - 0.02:
            out.append(n)
        elif n["pitch"] > last["pitch"]:
            last["end"] = max(last["start"] + 0.05, n["start"])
            out.append(n)
    return [n for n in out if n["end"] - n["start"] >= 0.06]


def monophony_rate(ns):
    if len(ns) < 2:
        return 1.0
    ns = sorted(ns, key=lambda n: n["start"])
    ok = sum(1 for a, b in zip(ns, ns[1:]) if a["end"] <= b["start"] + 0.02)
    return ok / (len(ns) - 1)


def cmd_pair_diff(args) -> dict:
    vnotes = extract_notes(args.vocal)
    inotes = extract_notes(args.inst)
    residual, matched = diff_notes(vnotes, inotes, args.tol)
    in_range = [n for n in residual if 48 <= n["pitch"] <= 84]  # C3–C6
    mono = mono_collapse(in_range)

    result = {
        "tool": "pi-audio/pair-diff",
        "vocal": args.vocal,
        "inst": args.inst,
        "vocal_notes": len(vnotes),
        "inst_notes": len(inotes),
        "matched_to_inst": matched,
        "match_rate": round(matched / max(1, len(vnotes)), 2),
        "residual": len(residual),
        "residual_in_C3_C6": len(in_range),
        "mono_notes": len(mono),
        "mono_rate": round(monophony_rate(mono), 2),
        "sv_importable_whole": len(mono) <= 512,  # import_monophonic_score 上限
        "note": "残差含和声/混音差异；单音化保留最高声部，低声部和声会被丢弃",
    }
    if mono:
        ps = [n["pitch"] for n in mono]
        result["mono_range"] = f"{note_name(min(ps))}-{note_name(max(ps))}"

    if args.midi:
        import pretty_midi

        pm = pretty_midi.PrettyMIDI()
        instr = pretty_midi.Instrument(program=54, name="vocal-mono")
        for n in mono:
            instr.notes.append(
                pretty_midi.Note(
                    velocity=max(1, min(127, int(n.get("velocity", 90)))),
                    pitch=n["pitch"],
                    start=n["start"],
                    end=n["end"],
                )
            )
        pm.instruments.append(instr)
        pm.write(args.midi)
        result["midi_out"] = args.midi

    return result


def main():
    ap = argparse.ArgumentParser(prog="pi-audio", description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("probe", help="特征指纹 + 可选 PANNs 判别")
    p.add_argument("audio")
    p.add_argument("--panns", action="store_true", help="加 PANNs 乐器/genre/有词判别（需 torch）")
    p.add_argument("--notes", action="store_true", help="加 basic-pitch 音符统计（慢 ~20s）")
    p.set_defaults(fn=cmd_probe)

    d = sub.add_parser("pair-diff", help="有词/无词配对差分 → 单音人声轨")
    d.add_argument("vocal")
    d.add_argument("inst")
    d.add_argument("--midi", help="导出单音化 MIDI 路径")
    d.add_argument("--tol", type=float, default=0.08, help="起始时间匹配容差秒 (默认 0.08)")
    d.set_defaults(fn=cmd_pair_diff)

    args = ap.parse_args()
    try:
        print(json.dumps(args.fn(args), ensure_ascii=False, indent=1))
    except Exception as e:  # 出错也保证输出合法 JSON，方便 agent 消费
        print(json.dumps({"tool": "pi-audio", "error": f"{type(e).__name__}: {e}"}, ensure_ascii=False))
        sys.exit(1)


if __name__ == "__main__":
    main()
