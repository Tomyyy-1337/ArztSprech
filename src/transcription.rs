use std::collections::VecDeque;
use std::error::Error;
use crate::audio::AudioSource;
use crate::protocol::Msg;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

pub const DEFAULT_MODEL: &str = "models/ggml-large-v3-turbo.bin";
const WHISPER_SAMPLE_RATE: u32 = 16_000;

// Pausenerkennung: 30-ms-Frames, ein Satz endet nach 0,5 s Stille oder spätestens nach 25 s.
const FRAME_LEN: usize = (WHISPER_SAMPLE_RATE as usize) * 30 / 1000;
const PRE_ROLL_FRAMES: usize = 15;
const END_SILENCE_FRAMES: usize = 17;
const MAX_UTTERANCE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 25;
const MIN_SPEECH_FRAMES: usize = 8;
const MIN_SPEECH_RMS: f32 = 0.004;
// Während des Sprechens wird der bisherige Satz in diesem Abstand erneut transkribiert.
const PARTIAL_INTERVAL_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 3 / 4;

pub struct Transcriber {
    ctx: WhisperContext,
    prompt: Option<String>,
}

impl Transcriber {
    pub fn load(model_path: &str) -> Result<Self, Box<dyn Error>> {
        // Optionaler Hinweistext für Fachbegriffe; kann Whisper auch zu Halluzinationen verleiten.
        let prompt = std::env::var("ARZTSPRECH_PROMPT").ok();

        // Nutzt die GPU, sofern ein im Build aktiviertes Backend (cuda/vulkan) ein Gerät findet, sonst die CPU.
        let mut ctx_params = WhisperContextParameters::default();
        let gpu_device = std::env::var("ARZTSPRECH_GPU_DEVICE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        ctx_params.use_gpu(true).flash_attn(true).gpu_device(gpu_device);
        let ctx = WhisperContext::new_with_params(model_path, ctx_params)?;
        Ok(Self { ctx, prompt })
    }
}

/// Transkribiert Audio aus `source`, bis dessen Kanal geschlossen wird.
pub fn run_transcriber(
    transcriber: &Transcriber,
    source: AudioSource,
    mut send: impl FnMut(Msg),
) -> Result<(), Box<dyn Error>> {
    let prompt = transcriber.prompt.as_deref();
    let mut state = transcriber.ctx.create_state()?;
    send(Msg::Status(format!("Bereit – {}", source.description)));

    let mut resampler = Resampler::new(source.sample_rate, WHISPER_SAMPLE_RATE);
    let mut segmenter = Segmenter::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut last_partial_len = 0;

    // Der Stream liefert Audio im Hintergrund; hier wird es in Frames zerlegt und transkribiert.
    while let Ok(chunk) = source.rx.recv() {
        pending.extend(resampler.process(&chunk));
        let mut frames = pending.chunks_exact(FRAME_LEN);
        for frame in frames.by_ref() {
            if let Some(utterance) = segmenter.push_frame(frame) {
                last_partial_len = 0;
                send(Msg::Final(transcribe(&mut state, &utterance, prompt.as_deref())?));
            }
        }
        pending = frames.remainder().to_vec();

        if segmenter.in_speech() && segmenter.utterance().len() >= last_partial_len + PARTIAL_INTERVAL_SAMPLES {
            last_partial_len = segmenter.utterance().len();
            send(Msg::Partial(transcribe(&mut state, segmenter.utterance(), prompt.as_deref())?));
        }
    }

    Ok(())
}

/// Erkennt Sprechpausen anhand der Lautstärke und liefert abgeschlossene Äußerungen.
struct Segmenter {
    pre_roll: VecDeque<Vec<f32>>,
    utterance: Vec<f32>,
    speech_frames: usize,
    silence_frames: usize,
    in_speech: bool,
    noise_floor: f32,
}

impl Segmenter {
    fn new() -> Self {
        Self {
            pre_roll: VecDeque::new(),
            utterance: Vec::new(),
            speech_frames: 0,
            silence_frames: 0,
            in_speech: false,
            noise_floor: 0.0,
        }
    }

    fn in_speech(&self) -> bool {
        self.in_speech
    }

    fn utterance(&self) -> &[f32] {
        &self.utterance
    }

    fn push_frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        let threshold = MIN_SPEECH_RMS.max(self.noise_floor * 2.5);
        let is_speech = rms > threshold;

        if !self.in_speech {
            if is_speech {
                self.in_speech = true;
                self.speech_frames = 1;
                self.silence_frames = 0;
                self.utterance = self.pre_roll.drain(..).flatten().collect();
                self.utterance.extend_from_slice(frame);
            } else {
                // Der Rauschpegel wird nur in Stille langsam nachgeführt.
                self.noise_floor = self.noise_floor * 0.95 + rms * 0.05;
                self.pre_roll.push_back(frame.to_vec());
                if self.pre_roll.len() > PRE_ROLL_FRAMES {
                    self.pre_roll.pop_front();
                }
            }
            return None;
        }

        self.utterance.extend_from_slice(frame);
        if is_speech {
            self.speech_frames += 1;
            self.silence_frames = 0;
        } else {
            self.silence_frames += 1;
        }

        let finished =
            self.silence_frames >= END_SILENCE_FRAMES || self.utterance.len() >= MAX_UTTERANCE_SAMPLES;
        if !finished {
            return None;
        }

        self.in_speech = false;
        let utterance = std::mem::take(&mut self.utterance);
        // Kurze Geräusche (Husten, Klicken) werden verworfen.
        (self.speech_frames >= MIN_SPEECH_FRAMES).then_some(utterance)
    }
}

/// Lineare Interpolation auf die von Whisper benötigte Abtastrate, blockweise nutzbar.
struct Resampler {
    ratio: f64,
    pos: f64,
    pending: Vec<f32>,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self { ratio: from as f64 / to as f64, pos: 0.0, pending: Vec::new() }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        while self.pos + 1.0 < self.pending.len() as f64 {
            let idx = self.pos as usize;
            let frac = (self.pos - idx as f64) as f32;
            let (a, b) = (self.pending[idx], self.pending[idx + 1]);
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        // Verbrauchte Samples verwerfen, die Nachkommastelle der Position bleibt erhalten.
        let consumed = (self.pos as usize).min(self.pending.len());
        self.pending.drain(..consumed);
        self.pos -= consumed as f64;
        out
    }
}

fn transcribe(state: &mut WhisperState, audio: &[f32], prompt: Option<&str>) -> Result<String, Box<dyn Error>> {
    // Zu kurze Aufnahmen (< 0,25 s) ergeben nur Halluzinationen.
    if audio.len() < (WHISPER_SAMPLE_RATE / 4) as usize {
        return Ok(String::new());
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    params.set_n_threads(threads as i32);
    params.set_language(Some("de"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_suppress_nst(true);
    if let Some(prompt) = prompt {
        params.set_initial_prompt(prompt);
    }
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    state.full(params, audio)?;

    let mut text = String::new();
    for segment in state.as_iter() {
        let segment = segment.to_str_lossy()?;
        let segment = segment.trim();
        // Geräusch-Annotationen wie "*Glubb*" oder "[Musik]" sind keine Sprache.
        let is_annotation = [('*', '*'), ('[', ']'), ('(', ')')]
            .iter()
            .any(|&(open, close)| segment.starts_with(open) && segment.ends_with(close));
        if !is_annotation {
            text.push_str(segment);
            text.push(' ');
        }
    }
    Ok(collapse_repeats(&text))
}

/// Entfernt Wiederholungsschleifen: Dasselbe Wort bleibt höchstens zweimal in Folge stehen.
fn collapse_repeats(text: &str) -> String {
    let key = |word: &str| word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let mut words: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        let run = words.iter().rev().take_while(|previous| key(previous) == key(word)).count();
        if run < 2 {
            words.push(word);
        }
    }
    words.join(" ")
}
