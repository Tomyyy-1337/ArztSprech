use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use crate::audio::AudioSource;
use crate::protocol::{Model, Msg};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

pub const DEFAULT_MODEL: &str = "models/german-turbo.bin";
const WHISPER_SAMPLE_RATE: u32 = 16_000;

impl Model {
    pub fn path(self) -> &'static str {
        match self {
            Model::Turbo => "models/ggml-large-v3-turbo.bin",
            Model::GermanTurbo => "models/german-turbo.bin",
        }
    }
}

// Pausenerkennung: 30-ms-Frames, eine Äußerung endet nach etwa 1,5 s Stille oder spätestens nach 120 s.
const FRAME_LEN: usize = (WHISPER_SAMPLE_RATE as usize) * 30 / 1000;
const PRE_ROLL_FRAMES: usize = 15;
const END_SILENCE_FRAMES: usize = 50;
const MAX_UTTERANCE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 120;
const MIN_SPEECH_FRAMES: usize = 8;
const MIN_SPEECH_RMS: f32 = 0.004;
// Stille am Ende wird bis auf diese Länge gekürzt, damit Whisper dort nichts erfindet.
const TRAIL_PAD_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 3 / 10;
// Segmente mit höherer no-speech-Wahrscheinlichkeit werden verworfen.
const NO_SPEECH_MAX: f32 = 0.6;
// Längste Wortfolge, die als Wiederholungsschleife erkannt wird.
const MAX_LOOP_WORDS: usize = 8;
// Während des Sprechens wird das offene Fenster in diesem Abstand neu transkribiert.
const PASS_INTERVAL_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 2;
// Dieser Abstand zum aktuellen Audio bleibt korrigierbar und wird nicht final gesendet.
const COMMIT_TAIL_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;
// Ab dieser Fensterlänge wird an bestätigten Wörtern abgeschnitten.
const MAX_WINDOW_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 10;
// Wörter, die nach einem Satzende folgen müssen, bevor der Satz final wird.
const SENTENCE_LOOKAHEAD_WORDS: usize = 5;
// Wörter, die nach einem Komma folgen müssen, bevor bis dahin final gesendet wird (ohne Audio abzuschneiden).
const COMMA_LOOKAHEAD_WORDS: usize = 7;
// Abkürzungen mit Punkt (ohne Punkte und in Kleinbuchstaben), die kein Satzende markieren.
const ABBREVIATIONS: &[&str] = &[
    "dr", "prof", "zb", "dh", "ua", "bzw", "bzgl", "ca", "nr", "hr", "fr", "bspw", "evtl", "ggf", "vgl",
    "inkl", "geb", "std", "tel", "mio", "u", "v",
];

/// Wort mit Endposition (in Samples relativ zum Fenster) des Whisper-Segments.
type Word = (String, usize);

pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn load(model_path: &str) -> Result<Self, Box<dyn Error>> {
        // Nutzt die GPU, sofern ein im Build aktiviertes Backend (cuda/vulkan) ein Gerät findet, sonst die CPU.
        let mut ctx_params = WhisperContextParameters::default();
        let gpu_device = std::env::var("ARZTSPRECH_GPU_DEVICE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        ctx_params.use_gpu(true).flash_attn(true).gpu_device(gpu_device);
        let ctx = WhisperContext::new_with_params(model_path, ctx_params)?;
        Ok(Self { ctx })
    }
}

/// Transkribiert Audio aus `source`, bis dessen Kanal geschlossen oder `stop` gesetzt wird.
pub fn run_transcriber(
    transcriber: &Transcriber,
    source: AudioSource,
    stop: &AtomicBool,
    prompt: Option<&str>,
    mut send: impl FnMut(Msg),
) -> Result<(), Box<dyn Error>> {
    let mut state = transcriber.ctx.create_state()?;
    send(Msg::Status(format!("Bereit – {}", source.description)));

    let mut resampler = Resampler::new(source.sample_rate, WHISPER_SAMPLE_RATE);
    let mut segmenter = Segmenter::new();
    let mut stream = Stream::default();
    let mut pending: Vec<f32> = Vec::new();

    // Der Stream liefert Audio im Hintergrund; hier wird es in Frames zerlegt und transkribiert.
    while !stop.load(Ordering::SeqCst) {
        let chunk = match source.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => chunk,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        pending.extend(resampler.process(&chunk));
        let mut frames = pending.chunks_exact(FRAME_LEN);
        for frame in frames.by_ref() {
            let was_speaking = segmenter.in_speech();
            match segmenter.push_frame(frame) {
                Some(utterance) => {
                    let words = transcribe(&mut state, &utterance[stream.offset..], prompt)?;
                    send(Msg::Final(join_words(&stream.remaining(&words))));
                    stream = Stream::default();
                }
                // Verworfenes Geräusch: der offene Rest wird zurückgenommen.
                None if was_speaking && !segmenter.in_speech() => {
                    send(Msg::Partial(String::new()));
                    stream = Stream::default();
                }
                None => {}
            }
        }
        pending = frames.remainder().to_vec();

        if segmenter.in_speech() && segmenter.utterance().len() >= stream.last_pass_len + PASS_INTERVAL_SAMPLES {
            stream.last_pass_len = segmenter.utterance().len();
            let window = &segmenter.utterance()[stream.offset..];
            let words = transcribe(&mut state, window, prompt)?;
            let (newly, tail) = stream.pass(words, window.len());
            if !newly.is_empty() {
                send(Msg::Final(join_words(&newly)));
            }
            send(Msg::Partial(join_words(&tail)));
        }
    }

    Ok(())
}

/// Rollendes Fenster der laufenden Äußerung mit Wörtern, die noch nicht bestätigt sind.
#[derive(Default)]
struct Stream {
    /// Samples der Äußerung, die bereits abgeschnitten wurden.
    offset: usize,
    /// Anzahl der Wörter im Fenster, die bereits als final gesendet wurden.
    committed: usize,
    /// Hypothese des letzten Durchlaufs.
    prev: Vec<Word>,
    last_pass_len: usize,
}

impl Stream {
    /// Vergleicht mit dem letzten Durchlauf; liefert neu bestätigte Wörter und den unbestätigten Rest.
    fn pass(&mut self, words: Vec<Word>, window_len: usize) -> (Vec<String>, Vec<String>) {
        let agreed = self.prev.iter().zip(&words).take_while(|(a, b)| word_key(&a.0) == word_key(&b.0)).count();
        let commit_before = window_len.saturating_sub(COMMIT_TAIL_SAMPLES);
        // "Absatz" wird sofort bestätigt, sonst kann Whisper es im nächsten Durchlauf wieder verwerfen.
        let absatz_end = words.iter().rposition(|w| word_key(&w.0) == "absatz").map_or(0, |i| i + 1);
        let mut stable = sentence_commit_point(&words[..agreed], &words).max(absatz_end);
        // Ohne Satzende hilft nur die Zeit, sonst wächst das Fenster unbegrenzt.
        if window_len > MAX_WINDOW_SAMPLES {
            stable = stable.max(words[..agreed].partition_point(|(_, end)| *end <= commit_before));
        }
        let newly = if stable > self.committed {
            let texts = words[self.committed..stable].iter().map(|w| w.0.clone()).collect();
            self.committed = stable;
            texts
        } else {
            Vec::new()
        };
        self.prev = words;

        // Abschneiden nur an einer Segmentgrenze, hinter der alle Wörter bestätigt sind.
        let cut_possible = stable > 0
            && self.committed == stable
            && (stable == self.prev.len() || self.prev[stable].1 > self.prev[stable - 1].1);
        if cut_possible && (window_len > MAX_WINDOW_SAMPLES || ends_sentence(&self.prev[stable - 1].0)) {
            let cut = self.prev[stable - 1].1;
            self.offset += cut;
            self.prev = self
                .prev
                .split_off(stable)
                .into_iter()
                .map(|(text, end)| (text, end.saturating_sub(cut)))
                .collect();
            self.committed = 0;
        }

        let tail = self.remaining(&self.prev);
        (newly, tail)
    }

    /// Wörter einer abschließenden Transkription, die noch nicht gesendet wurden.
    fn remaining(&self, words: &[Word]) -> Vec<String> {
        words.get(self.committed..).unwrap_or_default().iter().map(|w| w.0.clone()).collect()
    }
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

/// Kürzt Stille am Ende, lässt aber einen kurzen Puffer stehen; reine Stille ergibt leeres Audio.
fn trim_trailing_silence(audio: &[f32]) -> &[f32] {
    let mut end = audio.len();
    loop {
        if end < FRAME_LEN {
            return &audio[..0];
        }
        let frame = &audio[end - FRAME_LEN..end];
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME_LEN as f32).sqrt();
        if rms > MIN_SPEECH_RMS {
            break;
        }
        end -= FRAME_LEN;
    }
    &audio[..(end + TRAIL_PAD_SAMPLES).min(audio.len())]
}

fn transcribe(state: &mut WhisperState, audio: &[f32], prompt: Option<&str>) -> Result<Vec<Word>, Box<dyn Error>> {
    let audio = trim_trailing_silence(audio);
    // Zu kurze Aufnahmen (< 0,25 s) ergeben nur Halluzinationen.
    if audio.len() < (WHISPER_SAMPLE_RATE / 4) as usize {
        return Ok(Vec::new());
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    params.set_n_threads(threads as i32);
    params.set_language(Some("de"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_suppress_nst(true);
    params.set_no_speech_thold(NO_SPEECH_MAX);
    if let Some(prompt) = prompt {
        params.set_initial_prompt(prompt);
    }
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    state.full(params, audio)?;

    let mut words = Vec::new();
    for segment in state.as_iter() {
        if segment.no_speech_probability() > NO_SPEECH_MAX {
            continue;
        }
        let text = segment.to_str_lossy()?;
        let text = text.trim();
        // Geräusch-Annotationen wie "*Glubb*" oder "[Musik]" sind keine Sprache.
        let is_annotation = [('*', '*'), ('[', ']'), ('(', ')')]
            .iter()
            .any(|&(open, close)| text.starts_with(open) && text.ends_with(close));
        if is_annotation {
            continue;
        }
        // Whisper-Zeitstempel sind in 10-ms-Schritten angegeben.
        let end = (segment.end_timestamp().max(0) as usize * 160).min(audio.len());
        words.extend(text.split_whitespace().map(|word| (word.to_string(), end)));
    }
    let mut words = collapse_repeats(words);
    apply_paragraphs(&mut words);
    Ok(words)
}

/// Entfernt Wiederholungsschleifen: eine Wortfolge bleibt höchstens zweimal in Folge stehen.
fn collapse_repeats(words: Vec<Word>) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    for word in words {
        out.push(word);
        for n in 1..=MAX_LOOP_WORDS {
            let len = out.len();
            if len < 3 * n {
                continue;
            }
            let same = |a: usize, b: usize| (0..n).all(|k| word_key(&out[a + k].0) == word_key(&out[b + k].0));
            let looping = same(len - 3 * n, len - 2 * n) && same(len - 2 * n, len - n);
            if looping {
                out.truncate(len - n);
                break;
            }
        }
    }
    out
}

fn word_key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Setzt vor "Absatz" einen Punkt (statt Komma) und schreibt das folgende Wort groß.
/// "Doppelpunkt" wird zu ":" und das folgende Wort ebenfalls groß geschrieben.
fn apply_paragraphs(words: &mut [Word]) {
    for i in 0..words.len() {
        let key = word_key(&words[i].0);
        if key == "doppelpunkt" {
            if i > 0 && !matches!(word_key(&words[i - 1].0).as_str(), "absatz" | "doppelpunkt") {
                let prev = &mut words[i - 1].0;
                if prev.ends_with([',', ';']) {
                    prev.pop();
                }
            }
            words[i].0 = ":".to_string();
        } else if key != "absatz" {
            continue;
        } else if i > 0 && word_key(&words[i - 1].0) != "absatz" {
            let prev = &mut words[i - 1].0;
            if prev.ends_with(',') {
                prev.pop();
                prev.push('.');
            } else if !prev.ends_with(['.', '!', '?']) {
                prev.push('.');
            }
        }
        if let Some(next) = words.get_mut(i + 1) {
            if !matches!(word_key(&next.0).as_str(), "absatz" | "doppelpunkt") {
                next.0 = capitalize_first(&next.0);
            }
        }
    }
}

fn capitalize_first(word: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for c in word.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Verbindet Wörter und ersetzt "Absatz" (auch mit Satzzeichen am Rand) durch einen Zeilenumbruch.
fn join_words(words: &[String]) -> String {
    let mut out = String::new();
    for word in words {
        if word_key(word) == "absatz" {
            out.push('\n');
        } else {
            if !out.is_empty() && !out.ends_with('\n') && word != ":" {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

/// Wahr, wenn das Wort mit Satzzeichen endet, das kein Abkürzungspunkt ist.
fn ends_sentence(word: &str) -> bool {
    if word_key(word) == "absatz" {
        return true;
    }
    let trimmed = word.trim_end_matches(|c: char| matches!(c, '"' | '\'' | ')' | ']' | '»' | '“' | '”'));
    match trimmed.chars().last() {
        Some('!' | '?') => true,
        Some('.') => {
            let stem: String = trimmed.trim_end_matches('.').chars().filter(|c| c.is_alphabetic()).collect();
            !ABBREVIATIONS.contains(&stem.to_lowercase().as_str())
        }
        _ => false,
    }
}

/// Wahr, wenn das Wort mit einem Komma endet.
fn ends_comma(word: &str) -> bool {
    word.trim_end_matches(|c: char| matches!(c, '"' | '\'' | ')' | ']' | '»' | '“' | '”')).ends_with(',')
}

/// Länge des Präfixes bis zum letzten bestätigten Satzende bzw. Komma, dem in `words` genug Wörter folgen.
fn sentence_commit_point(agreed: &[Word], words: &[Word]) -> usize {
    agreed
        .iter()
        .enumerate()
        .rev()
        .find(|(i, (text, _))| {
            let after = words.len() - (*i + 1);
            (ends_sentence(text) && after >= SENTENCE_LOOKAHEAD_WORDS) || (ends_comma(text) && after >= COMMA_LOOKAHEAD_WORDS)
        })
        .map_or(0, |(i, _)| i + 1)
}