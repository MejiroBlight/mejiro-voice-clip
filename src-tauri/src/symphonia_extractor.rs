use std::fs::File;
use std::path::Path;

use anyhow::Context;
use hound::{SampleFormat, WavSpec, WavWriter};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};
use symphonia_adapter_fdk_aac::AacDecoder;

pub fn extract_audio_to_wav(input: &Path, output: &Path) -> anyhow::Result<()> {
    let file = File::open(input).context("opening input file")?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(
        input
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default(),
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
        .ok_or_else(|| anyhow::anyhow!("no supported audio track found (the file may not contain audio, or the codec is unsupported)"))?;

    let mut decoder: Box<dyn symphonia::core::codecs::Decoder> =
        if track.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_AAC {
            Box::new(AacDecoder::try_new(
                &track.codec_params,
                &DecoderOptions::default(),
            )?)
        } else {
            get_codecs().make(&track.codec_params, &DecoderOptions::default())?
        };

    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    // Decode all packets into a single sample buffer (i16) first.
    let mut all_samples = Vec::<i16>::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        if packet.track_id() != track.id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::IoError(_)) => continue,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.into()),
        };

        let mut sample_buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buf.copy_interleaved_ref(decoded);

        all_samples.extend_from_slice(sample_buf.samples());
    }

    // Split into two halves.
    let half = all_samples.len() / 2;
    let (first, second) = all_samples.split_at(half);

    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let dir = output.parent().unwrap_or_else(|| Path::new("."));

    let out1 = dir.join(format!("{}_part1.wav", stem));
    let out2 = dir.join(format!("{}_part2.wav", stem));

    write_wav(&out1, spec, first)?;
    write_wav(&out2, spec, second)?;

    Ok(())
}

fn write_wav(output: &Path, spec: WavSpec, samples: &[i16]) -> anyhow::Result<()> {
    let mut writer = WavWriter::create(output, spec)?;
    for sample in samples {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(())
}
