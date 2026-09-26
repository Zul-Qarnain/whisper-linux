use anyhow::{bail, Context, Result};
use std::{fs, io::Write, path::Path};

pub const TARGET_RATE: usize = 16_000;

/// Reads a WAV file, mixes to mono f32 in -1..1 and resamples to 16 kHz.
pub fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let data = fs::read(path).context("read wav")?;
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file: {}", path.display());
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (tag, channels, rate, bits)
    let mut pcm: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?) as usize;
        let body = &data[pos + 8..(pos + 8 + len).min(data.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let tag = u16::from_le_bytes(body[0..2].try_into()?);
                let channels = u16::from_le_bytes(body[2..4].try_into()?);
                let rate = u32::from_le_bytes(body[4..8].try_into()?);
                let bits = u16::from_le_bytes(body[14..16].try_into()?);
                let tag = if tag == 0xFFFE && body.len() >= 26 {
                    u16::from_le_bytes(body[24..26].try_into()?)
                } else {
                    tag
                };
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => pcm = Some(body),
            _ => {}
        }
        pos = pos + 8 + len + (len % 2);
    }

    let (tag, channels, rate, bits) = fmt.context("no fmt chunk")?;
    let pcm = pcm.context("no data chunk")?;
    if channels == 0 {
        bail!("invalid channel count");
    }

    let mono = decode_pcm(pcm, channels as usize, tag, bits)?;
    if rate as usize == TARGET_RATE {
        Ok(mono)
    } else {
        Ok(resample_linear(&mono, rate as usize, TARGET_RATE))
    }
}

fn decode_pcm(pcm: &[u8], channels: usize, tag: u16, bits: u16) -> Result<Vec<f32>> {
    let read_i = |byte: usize, n: usize| -> i64 {
        let mut v: u64 = 0;
        for i in 0..n {
            v |= (pcm[byte + i] as u64) << (8 * i);
        }
        let sign = 1u64 << (8 * n - 1);
        if v & sign != 0 {
            v.wrapping_sub(1u64 << (8 * n)) as i64
        } else {
            v as i64
        }
    };
    let full = move |s: i64, nbits: u32| s as f32 / (1i64 << (nbits - 1)) as f32;

    match (tag, bits) {
        (1, 16) | (0xFFFE, 16) => {
            let frames = pcm.len() / 2 / channels;
            Ok((0..frames)
                .map(|f| (0..channels).map(|c| full(read_i((f * channels + c) * 2, 2), 16)).sum::<f32>() / channels as f32)
                .collect())
        }
        (1, 24) => {
            let frames = pcm.len() / 3 / channels;
            Ok((0..frames)
                .map(|f| (0..channels).map(|c| full(read_i((f * channels + c) * 3, 3), 24)).sum::<f32>() / channels as f32)
                .collect())
        }
        (1, 32) => {
            let frames = pcm.len() / 4 / channels;
            Ok((0..frames)
                .map(|f| (0..channels).map(|c| full(read_i((f * channels + c) * 4, 4), 32)).sum::<f32>() / channels as f32)
                .collect())
        }
        (3, 32) => {
            let frames = pcm.len() / 4 / channels;
            Ok((0..frames)
                .map(|f| {
                    (0..channels)
                        .map(|c| {
                            let b: [u8; 4] = pcm[(f * channels + c) * 4..(f * channels + c) * 4 + 4].try_into().unwrap();
                            f32::from_le_bytes(b).clamp(-1.0, 1.0)
                        })
                        .sum::<f32>()
                        / channels as f32
                })
                .collect())
        }
        _ => bail!("unsupported wav format tag={tag} bits={bits} (need PCM 16/24/32 or IEEE float32)"),
    }
}

/// Linear-interpolation resampler (voice-band quality is sufficient for ASR).
pub fn resample_linear(input: &[f32], from_rate: usize, to_rate: usize) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let i0 = src as usize;
        let frac = (src - i0 as f64) as f32;
        let a = input[i0.min(input.len() - 1)];
        let b = input[(i0 + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

pub fn write_wav_pcm16(path: &Path, samples: &[f32], rate: usize) -> Result<()> {
    let mut file = fs::File::create(path)?;
    let data_len = samples.len() * 2;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len as u32).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&(rate as u32).to_le_bytes())?;
    file.write_all(&(rate as u32 * 2).to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&(data_len as u32).to_le_bytes())?;
    let mut pcm = Vec::with_capacity(data_len);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    file.write_all(&pcm)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_pcm16_wav(rate: usize, secs: usize) -> Vec<u8> {
        let n = rate * secs;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(rate as u32).to_le_bytes());
        out.extend_from_slice(&(rate as u32 * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&((n * 2) as u32).to_le_bytes());
        for i in 0..n {
            let s = (i as f32 * 0.05).sin() * 8000.0;
            out.extend_from_slice(&(s as i16).to_le_bytes());
        }
        out
    }

    #[test]
    fn reads_and_resamples_48k_pcm16() {
        let path = std::env::temp_dir().join("wf-wav-48k.wav");
        fs::write(&path, synth_pcm16_wav(48_000, 1)).unwrap();
        let mono = read_wav_16k_mono(&path).unwrap();
        assert!((mono.len() as f64 - 16000.0).abs() < 2.0);
        assert!(mono.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn stereo_mixes_to_mono() {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + 8 * 2) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16000u32.to_le_bytes());
        out.extend_from_slice(&(16000u32 * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&16u32.to_le_bytes());
        for _ in 0..4 {
            out.extend_from_slice(&1000i16.to_le_bytes());
            out.extend_from_slice(&3000i16.to_le_bytes());
        }
        let path = std::env::temp_dir().join("wf-wav-stereo.wav");
        fs::write(&path, &out).unwrap();
        let mono = read_wav_16k_mono(&path).unwrap();
        assert_eq!(mono.len(), 4);
        let expect = (1000.0 + 3000.0) / 2.0 / 32768.0;
        assert!((mono[0] - expect).abs() < 1e-4);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let samples: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let path = std::env::temp_dir().join("wf-wav-rt.wav");
        write_wav_pcm16(&path, &samples, TARGET_RATE).unwrap();
        let back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(back.len(), samples.len());
        let err = samples.iter().zip(&back).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 0.001);
    }
}
