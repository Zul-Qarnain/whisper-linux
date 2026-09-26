pub struct VoiceActivityDetector {
    threshold: f32,
    pub silence_chunks: usize,
    pub speech_active: bool,
}

impl VoiceActivityDetector {
    pub fn new(threshold: f32) -> Self {
        Self { threshold, silence_chunks: 0, speech_active: false }
    }

    /// Feed one chunk of mono f32 samples; returns RMS level for meters.
    pub fn feed(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        if rms >= self.threshold {
            self.silence_chunks = 0;
            self.speech_active = true;
        } else if self.speech_active {
            self.silence_chunks += 1;
        }
        rms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_speech_then_endpoints_on_silence() {
        let mut vad = VoiceActivityDetector::new(0.01);
        let loud: Vec<f32> = (0..160).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let quiet = vec![0.0f32; 160];

        vad.feed(&quiet);
        assert!(!vad.speech_active);
        vad.feed(&loud);
        assert!(vad.speech_active);
        for _ in 0..3 {
            vad.feed(&quiet);
        }
        assert_eq!(vad.silence_chunks, 3);
        assert!(vad.speech_active); // stays active until caller endpoints on silence_chunks
    }
}
