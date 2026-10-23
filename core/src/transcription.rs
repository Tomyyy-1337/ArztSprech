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
// Während des Sprechens wird das offene Fenster in diesem Abstand neu transkribiert.
const PASS_INTERVAL_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 2;
// Dieser Abstand zum aktuellen Audio bleibt korrigierbar und wird nicht final gesendet.
const COMMIT_TAIL_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;
// Ab dieser Fensterlänge wird an bestätigten Wörtern abgeschnitten.
const MAX_WINDOW_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 10;
// Bestätigte Wörter, die nach einem Satzende folgen müssen, bevor der Satz final wird.
const SENTENCE_LOOKAHEAD_WORDS: usize = 2;
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
        let stable = words[..agreed]
            .partition_point(|(_, end)| *end <= commit_before)
            .max(sentence_commit_point(&words[..agreed]));
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

fn transcribe(state: &mut WhisperState, audio: &[f32], prompt: Option<&str>) -> Result<Vec<Word>, Box<dyn Error>> {
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

/// Entfernt Wiederholungsschleifen: Dasselbe Wort bleibt höchstens zweimal in Folge stehen.
fn collapse_repeats(words: Vec<Word>) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    for word in words {
        let key = word_key(&word.0);
        let run = out.iter().rev().take_while(|previous| word_key(&previous.0) == key).count();
        if run < 2 {
            out.push(word);
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

/// Länge des Präfixes bis zum letzten Satzende, dem genug bestätigte Wörter folgen oder das "Absatz" ist.
fn sentence_commit_point(agreed: &[Word]) -> usize {
    agreed
        .iter()
        .enumerate()
        .rev()
        .find(|(i, (text, _))| {
            ends_sentence(text)
                && (word_key(text) == "absatz" || agreed.len() - (*i + 1) >= SENTENCE_LOOKAHEAD_WORDS)
        })
        .map_or(0, |(i, _)| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[(&str, usize)]) -> Vec<Word> {
        list.iter().map(|(text, end)| (text.to_string(), *end)).collect()
    }

    #[test]
    fn sentence_end_detection() {
        assert!(ends_sentence("Ende."));
        assert!(ends_sentence("Wirklich?"));
        assert!(ends_sentence("gut!\""));
        assert!(!ends_sentence("Dr."));
        assert!(!ends_sentence("z.B."));
        assert!(!ends_sentence("Haus"));
    }

    #[test]
    fn sentence_commits_after_lookahead() {
        let mut stream = Stream::default();
        let base = words(&[("Hallo.", 8000), ("Wie", 16000), ("geht", 24000), ("es", 32000), ("dir", 40000)]);
        stream.pass(base, 16000);
        let longer = words(&[
            ("Hallo.", 8000),
            ("Wie", 16000),
            ("geht", 24000),
            ("es", 32000),
            ("dir", 40000),
            ("gut?", 48000),
        ]);
        let (newly, tail) = stream.pass(longer, 16000);
        assert_eq!(newly, vec!["Hallo.".to_string()]);
        assert_eq!(tail[0], "Wie");
        assert_eq!(stream.offset, 8000);
    }

    #[test]
    fn absatz_commits_without_lookahead() {
        let agreed = words(&[("Hallo,", 8000), ("Absatz", 16000)]);
        assert_eq!(sentence_commit_point(&agreed), 2);
    }

    #[test]
    fn no_commit_without_lookahead() {
        let mut stream = Stream::default();
        let three = words(&[("Hallo.", 8000), ("Wie", 16000), ("geht", 24000)]);
        stream.pass(three.clone(), 16000);
        let (newly, _) = stream.pass(three, 16000);
        assert!(newly.is_empty());
    }

    #[test]
    fn absatz_paragraph_rules() {
        let mut list = words(&[("Hallo,", 1), ("Absatz,", 2), ("neu", 3), ("Dr", 4), ("Absatz", 5)]);
        apply_paragraphs(&mut list);
        let texts: Vec<&str> = list.iter().map(|w| w.0.as_str()).collect();
        assert_eq!(texts, ["Hallo.", "Absatz,", "Neu", "Dr.", "Absatz"]);
    }

    #[test]
    fn doppelpunkt_becomes_colon_and_capitalizes() {
        let mut list = words(&[("Hallo,", 1), ("Doppelpunkt", 2), ("heute", 3)]);
        apply_paragraphs(&mut list);
        let texts: Vec<&str> = list.iter().map(|w| w.0.as_str()).collect();
        assert_eq!(texts, ["Hallo", ":", "Heute"]);
        let joined: Vec<String> = list.iter().map(|w| w.0.clone()).collect();
        assert_eq!(join_words(&joined), "Hallo: Heute");
    }

    #[test]
    fn absatz_becomes_line_break() {
        let list = vec!["Hallo,".to_string(), "Absatz,".to_string(), "Neu".to_string(), "ABSATZ".to_string()];
        assert_eq!(join_words(&list), "Hallo,\nNeu\n");
    }

    #[test]
    fn short_pause_keeps_utterance() {
        let speech = vec![0.1f32; FRAME_LEN];
        let silence = vec![0.0f32; FRAME_LEN];
        let mut segmenter = Segmenter::new();
        for _ in 0..8 {
            assert!(segmenter.push_frame(&speech).is_none());
        }
        for _ in 0..20 {
            assert!(segmenter.push_frame(&silence).is_none());
        }
        for _ in 0..8 {
            assert!(segmenter.push_frame(&speech).is_none());
        }
        let finished = (0..END_SILENCE_FRAMES).find_map(|_| segmenter.push_frame(&silence));
        assert!(finished.is_some());
    }
}