//! Optional end-to-end smoke test against an FFmpeg already available on PATH.

use std::{f32::consts::TAU, fs, io::Write, path::Path, sync::atomic::AtomicBool};

use pi_agent_components::{
    AudioExecutor, ComponentPaths, FfmpegResolver, LoudnessTarget, NormalizeRequest,
    PrepareWavRequest, SampleFormat, SystemProcessRunner,
};

fn write_test_wav(path: &Path) {
    const SAMPLE_RATE: u32 = 16_000;
    const SAMPLES: u32 = SAMPLE_RATE;
    const DATA_BYTES: u32 = SAMPLES * 2;
    let mut file = fs::File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + DATA_BYTES).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&SAMPLE_RATE.to_le_bytes()).unwrap();
    file.write_all(&(SAMPLE_RATE * 2).to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&DATA_BYTES.to_le_bytes()).unwrap();
    for sample in 0..SAMPLES {
        let phase = sample as f32 / SAMPLE_RATE as f32 * 440.0 * TAU;
        let value = (phase.sin() * 0.2 * i16::MAX as f32) as i16;
        file.write_all(&value.to_le_bytes()).unwrap();
    }
}

#[test]
#[ignore = "requires ffmpeg and ffprobe on PATH"]
fn system_ffmpeg_runs_all_whitelisted_operations() {
    let root = std::env::temp_dir().join(format!("pi-agent-system-ffmpeg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("输入 dry voice.wav");
    write_test_wav(&input);

    let paths = ComponentPaths::from_data_root(root.clone());
    let resolved = FfmpegResolver::new(paths.clone()).resolve_system().unwrap();
    let executor = AudioExecutor::new(paths, resolved, SystemProcessRunner);
    let cancelled = AtomicBool::new(false);

    let probe = executor.probe(&input, &cancelled).unwrap();
    assert_eq!(probe.audio_streams[0].sample_rate, Some(16_000));

    let prepared = executor
        .prepare_wav(
            &PrepareWavRequest {
                input: input.clone(),
                output_name: Some("prepared 中文.wav".into()),
                start_seconds: Some(0.1),
                duration_seconds: Some(0.5),
                sample_rate: Some(44_100),
                channels: Some(2),
                sample_format: SampleFormat::S24,
            },
            &cancelled,
        )
        .unwrap();
    assert!(prepared.output.is_file());

    let target = LoudnessTarget {
        integrated_lufs: -16.0,
        true_peak_db: -1.5,
        loudness_range: 7.0,
    };
    let measured = executor
        .loudness_analyze(&input, &target, &cancelled)
        .unwrap();
    assert!(measured.integrated_lufs.is_finite());

    let normalized = executor
        .loudness_normalize(
            &NormalizeRequest {
                input,
                output_name: Some("normalized.wav".into()),
                target,
            },
            &cancelled,
        )
        .unwrap();
    assert!(normalized.output.is_file());

    fs::remove_dir_all(root).unwrap();
}
