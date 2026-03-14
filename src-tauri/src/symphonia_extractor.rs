use std::fs::File;
use std::path::Path;

use anyhow::Context;
use hound::{SampleFormat, WavSpec, WavWriter};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time as SymphoniaTime;
use symphonia::default::{get_codecs, get_probe};
use serde::Deserialize;
use symphonia_adapter_fdk_aac::AacDecoder;

#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub name: String,
    pub start: f64,
    pub end: f64,
}

/// リージョンの時刻範囲だけをデコードして WAV に書き出す。
/// ファイル全体をメモリに展開しない。
pub fn extract_region(input: &Path, output: &Path, region: Region) -> anyhow::Result<()> {
    let file = File::open(input).context("opening input file")?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(input.extension().and_then(|s| s.to_str()).unwrap_or_default());

    let probed = get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .cloned()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no supported audio track found"))?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);
    let time_base = track.codec_params.time_base;

    let mut decoder: Box<dyn symphonia::core::codecs::Decoder> =
        if track.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_AAC {
            Box::new(AacDecoder::try_new(&track.codec_params, &DecoderOptions::default())?)
        } else {
            get_codecs().make(&track.codec_params, &DecoderOptions::default())?
        };

    // コンテナレベルでリージョン開始付近にシーク（キーフレーム境界）
    let seek_time = SymphoniaTime::new(region.start as u64, region.start.fract());
    let _ = format.seek(
        SeekMode::Coarse,
        SeekTo::Time { time: seek_time, track_id: Some(track_id) },
    );
    decoder.reset();

    // サンプル単位の開始・終了フレーム
    let start_sample = (region.start * sample_rate as f64).round() as u64;
    let end_sample = (region.end * sample_rate as f64).round() as u64;

    let mut file_name = region.name.clone();
    if !file_name.to_lowercase().ends_with(".wav") {
        file_name.push_str(".wav");
    }
    let out_dir = output.parent().unwrap_or_else(|| Path::new("."));
    let out_path = out_dir.join(file_name);

    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&out_path, spec)?;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => { decoder.reset(); continue; }
            Err(err) => return Err(err.into()),
        };

        if packet.track_id() != track_id { continue; }

        // パケット先頭のサンプル位置を計算
        let pkt_start_sample: u64 = if let Some(tb) = time_base {
            let t = tb.calc_time(packet.ts());
            ((t.seconds as f64 + t.frac) * sample_rate as f64).round() as u64
        } else {
            packet.ts()
        };

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.into()),
        };

        let num_frames = decoded.frames() as u64;
        let pkt_end_sample = pkt_start_sample + num_frames;

        // リージョン開始より前のパケットはスキップ
        if pkt_end_sample <= start_sample { continue; }
        // リージョン終了を超えたら完了
        if pkt_start_sample >= end_sample { break; }

        let mut sample_buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();

        // パケット内でのサンプル単位トリミング
        let ch = channels as u64;
        let trim_start = if pkt_start_sample < start_sample {
            ((start_sample - pkt_start_sample) * ch) as usize
        } else { 0 };
        let trim_end = if pkt_end_sample > end_sample {
            ((pkt_end_sample - end_sample) * ch) as usize
        } else { 0 };

        let end_idx = samples.len().saturating_sub(trim_end);
        if trim_start < end_idx {
            for s in &samples[trim_start..end_idx] {
                writer.write_sample(*s)?;
            }
        }
    }

    writer.finalize()?;
    Ok(())
}

/// ファイルをチャンク単位でデコードし、chunk_secs 秒分のピークが溜まるたびに
/// コールバックへ通知するストリーミングピーク生成。
/// on_chunk(peaks, offset, total, duration_secs, is_done)
pub fn generate_peaks_streaming(
    input: &Path,
    peaks_count: usize,
    chunk_secs: f64,
    cancel: &std::sync::atomic::AtomicBool,
    mut on_chunk: impl FnMut(Vec<f32>, usize, usize, f64, bool),
) -> anyhow::Result<()> {
    let file = File::open(input).context("opening input file")?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(
        input.extension().and_then(|s| s.to_str()).unwrap_or_default(),
    );

    let probed = get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .cloned()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no supported audio track found"))?;

    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100) as f64;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2);
    let n_frames = track.codec_params.n_frames;

    let duration_secs = n_frames
        .map(|n| n as f64 / sample_rate)
        .unwrap_or(0.0);

    // フレームあたりのウィンドウサイズ (何フレームで1ピーク)
    let window_size = n_frames
        .map(|n| ((n as usize) / peaks_count).max(1))
        .unwrap_or((sample_rate as usize / peaks_count).max(1));

    // 総ピーク数の推定値
    let total_peaks = n_frames
        .map(|n| (n as usize + window_size - 1) / window_size)
        .unwrap_or(peaks_count);

    // chunk_secs 秒分に相当するピーク数
    let peaks_per_chunk =
        ((chunk_secs * sample_rate) / window_size as f64).ceil() as usize;
    let peaks_per_chunk = peaks_per_chunk.max(1);

    let mut decoder: Box<dyn symphonia::core::codecs::Decoder> =
        if track.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_AAC {
            Box::new(AacDecoder::try_new(
                &track.codec_params,
                &DecoderOptions::default(),
            )?)
        } else {
            get_codecs().make(&track.codec_params, &DecoderOptions::default())?
        };

    let mut window_max = 0.0f32;
    let mut window_frames = 0usize;
    let mut chunk: Vec<f32> = Vec::with_capacity(peaks_per_chunk);
    let mut global_offset = 0usize;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        // 新しい生成が始まったらキャンセルフラグが立つ → 黙って終了
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }

        if packet.track_id() != track.id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.into()),
        };

        let mut sample_buf =
            SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();

        let mut i = 0;
        while i + channels <= samples.len() {
            let mut frame_max = 0.0f32;
            for ch in 0..channels {
                frame_max = frame_max.max(samples[i + ch].abs());
            }
            window_max = window_max.max(frame_max);
            window_frames += 1;

            if window_frames >= window_size {
                chunk.push(window_max);
                window_max = 0.0;
                window_frames = 0;

                if chunk.len() >= peaks_per_chunk {
                    let offset = global_offset;
                    global_offset += chunk.len();
                    on_chunk(
                        std::mem::replace(&mut chunk, Vec::with_capacity(peaks_per_chunk)),
                        offset,
                        total_peaks,
                        duration_secs,
                        false,
                    );
                }
            }
            i += channels;
        }
    }

    // 最後のウィンドウ残り
    if window_frames > 0 {
        chunk.push(window_max);
    }

    // 最終チャンク送信
    let offset = global_offset;
    on_chunk(chunk, offset, total_peaks, duration_secs, true);

    Ok(())
}
