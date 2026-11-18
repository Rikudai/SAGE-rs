//! Captura de microfone push-to-talk e modo toque via `cpal`, detecção de silêncio,
//! reamostragem / downmix para 16 kHz mono 16-bit e codificação WAV via `hound`.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use tokio::sync::oneshot;

/// Taxa de amostragem alvo para transcrição (16 kHz).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Tempo de silêncio após fala para parada automática no modo toque (2,5 s).
pub const DEFAULT_SILENCE_TIMEOUT: Duration = Duration::from_millis(2500);

/// Limite máximo de duração de uma gravação (120 s).
pub const MAX_RECORD_DURATION: Duration = Duration::from_secs(120);

/// Limiar de energia RMS (em escala `i16`, `0..=32767`) para considerar fala ativa.
pub const DEFAULT_RMS_THRESHOLD: f32 = 250.0;

/// Detector de silêncio determinístico para modo toque e limite máximo de gravação.
///
/// Aciona parada automática quando:
/// 1. O tempo total acumulado atinge `max_duration` (120 s); ou
/// 2. Após ter detectado fala (`rms > rms_threshold`), o áudio permanece em silêncio
///    por pelo menos `silence_timeout` (2,5 s).
#[derive(Debug, Clone)]
pub struct SilenceDetector {
    pub rms_threshold: f32,
    pub silence_timeout: Duration,
    pub max_duration: Duration,
    speech_detected: bool,
    silence_elapsed: Duration,
    total_elapsed: Duration,
    peak_rms: f32,
}

impl Default for SilenceDetector {
    fn default() -> Self {
        Self::new(
            DEFAULT_RMS_THRESHOLD,
            DEFAULT_SILENCE_TIMEOUT,
            MAX_RECORD_DURATION,
        )
    }
}

impl SilenceDetector {
    pub fn new(rms_threshold: f32, silence_timeout: Duration, max_duration: Duration) -> Self {
        Self {
            rms_threshold,
            silence_timeout,
            max_duration,
            speech_detected: false,
            silence_elapsed: Duration::ZERO,
            total_elapsed: Duration::ZERO,
            peak_rms: 0.0,
        }
    }

    /// Calcula a energia RMS de um bloco de amostras `i16`.
    pub fn compute_rms(samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples
            .iter()
            .map(|&s| {
                let v = s as f64;
                v * v
            })
            .sum();
        (sum_sq / (samples.len() as f64)).sqrt() as f32
    }

    /// Alimenta um bloco de amostras mono e devolve `true` se a captura deve encerrar.
    pub fn feed_samples(&mut self, samples: &[i16], sample_rate: u32) -> bool {
        if samples.is_empty() || sample_rate == 0 {
            return self.total_elapsed >= self.max_duration;
        }
        let dt = Duration::from_secs_f64(samples.len() as f64 / sample_rate as f64);
        let rms = Self::compute_rms(samples);
        self.feed_rms(rms, dt)
    }

    /// Alimenta um valor RMS medido durante `dt` e devolve `true` se a captura deve encerrar.
    pub fn feed_rms(&mut self, rms: f32, dt: Duration) -> bool {
        self.total_elapsed = self.total_elapsed.saturating_add(dt);
        if rms > self.peak_rms {
            self.peak_rms = rms;
        }

        if self.total_elapsed >= self.max_duration {
            return true;
        }

        if rms > self.rms_threshold {
            self.speech_detected = true;
            self.silence_elapsed = Duration::ZERO;
            false
        } else if self.speech_detected {
            self.silence_elapsed = self.silence_elapsed.saturating_add(dt);
            self.silence_elapsed >= self.silence_timeout
        } else {
            false
        }
    }

    pub fn speech_detected(&self) -> bool {
        self.speech_detected
    }

    pub fn silence_elapsed(&self) -> Duration {
        self.silence_elapsed
    }

    pub fn total_elapsed(&self) -> Duration {
        self.total_elapsed
    }

    pub fn peak_rms(&self) -> f32 {
        self.peak_rms
    }
}

/// Converte amostras intercaladas `f32` (`-1.0..=1.0`) de `channels` canais para mono `i16`.
pub fn downmix_f32_to_mono_i16(samples: &[f32], channels: u16) -> Vec<i16> {
    let ch = (channels.max(1)) as usize;
    let mut out = Vec::with_capacity(samples.len() / ch);
    for frame in samples.chunks_exact(ch) {
        let sum: f32 = frame.iter().copied().sum();
        let avg = (sum / ch as f32).clamp(-1.0, 1.0);
        let scaled = (avg * 32767.0).round() as i32;
        out.push(scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
    }
    out
}

/// Converte amostras intercaladas `i16` de `channels` canais para mono `i16`.
pub fn downmix_i16_to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    let ch = (channels.max(1)) as usize;
    if ch == 1 {
        return samples.to_vec();
    }
    let mut out = Vec::with_capacity(samples.len() / ch);
    for frame in samples.chunks_exact(ch) {
        let sum: i32 = frame.iter().map(|&s| s as i32).sum();
        out.push((sum / ch as i32) as i16);
    }
    out
}

/// Converte amostras intercaladas `u16` de `channels` canais para mono `i16`.
pub fn downmix_u16_to_mono_i16(samples: &[u16], channels: u16) -> Vec<i16> {
    let ch = (channels.max(1)) as usize;
    let mut out = Vec::with_capacity(samples.len() / ch);
    for frame in samples.chunks_exact(ch) {
        let sum: i32 = frame.iter().map(|&u| (u as i32) - 32768).sum();
        out.push((sum / ch as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16);
    }
    out
}

/// Reamostra áudio mono `i16` de `src_rate` para `dst_rate` usando interpolação linear.
pub fn resample_mono(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if samples.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as u64 * dst_rate as u64) / src_rate as u64) as usize;
    if out_len == 0 {
        return Vec::new();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let last_idx = samples.len() - 1;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = (i as f64) * ratio;
        let idx = (src_pos.floor() as usize).min(last_idx);
        let frac = src_pos - (idx as f64);
        let s0 = samples[idx] as f64;
        let s1 = samples[(idx + 1).min(last_idx)] as f64;
        let interp = (s0 + (s1 - s0) * frac).round();
        out.push(interp.clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }
    out
}

/// Codifica amostras `i16` mono em um arquivo WAV de 16 bits e 16 kHz em memória.
pub fn encode_wav_16k_mono(samples: &[i16]) -> anyhow::Result<Vec<u8>> {
    encode_wav_mono(samples, TARGET_SAMPLE_RATE)
}

/// Codifica amostras `i16` mono em um arquivo WAV PCM de 16 bits na taxa informada.
pub fn encode_wav_mono(samples: &[i16], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::with_capacity(44 + samples.len() * 2));
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}

struct ActiveSession {
    stop_flag: Arc<AtomicBool>,
    rx: oneshot::Receiver<anyhow::Result<Vec<u8>>>,
}

/// Gravador de áudio em thread dedicada (`Send + Sync`, pois `cpal::Stream` é `!Send`).
#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<ActiveSession>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Retorna `true` se há uma captura ativa no momento.
    pub fn is_recording(&self) -> bool {
        self.session
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Inicia gravação push-to-talk (encerra em `stop()` ou no teto de 120 s).
    pub fn start(&self) -> anyhow::Result<()> {
        self.start_with_mode(false)
    }

    /// Inicia gravação no dispositivo de entrada padrão.
    /// Quando `tap_mode == true`, encerra automaticamente após 2,5 s de silêncio depois da fala.
    pub fn start_with_mode(&self, tap_mode: bool) -> anyhow::Result<()> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("Mutex do gravador envenenado"))?;

        // Se já havia uma sessão anterior, sinaliza parada antes de abrir uma nova.
        if let Some(prev) = guard.take() {
            prev.stop_flag.store(true, Ordering::SeqCst);
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);
        let (tx, rx) = oneshot::channel::<anyhow::Result<Vec<u8>>>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<()>>(1);

        std::thread::Builder::new()
            .name("sage-recorder".into())
            .spawn(move || {
                run_capture_thread(tap_mode, stop_flag_thread, init_tx, tx);
            })
            .map_err(|e| anyhow::anyhow!("Falha ao criar thread de gravação: {e}"))?;

        match init_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                *guard = Some(ActiveSession { stop_flag, rx });
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => anyhow::bail!("Tempo esgotado ao inicializar o microfone padrão"),
        }
    }

    /// Sinaliza o fim da gravação e aguarda de forma assíncrona os bytes WAV (16 kHz, mono, 16-bit).
    pub async fn stop_async(&self) -> anyhow::Result<Vec<u8>> {
        let active = {
            let mut guard = self
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("Mutex do gravador envenenado"))?;
            guard.take()
        };
        let Some(active) = active else {
            anyhow::bail!("Nenhuma gravação ativa para encerrar");
        };
        active.stop_flag.store(true, Ordering::SeqCst);
        active
            .rx
            .await
            .map_err(|_| anyhow::anyhow!("Thread de captura encerrou inesperadamente"))?
    }

    /// Aguarda o término natural da captura (silêncio no modo toque, limite de 120 s ou `stop()`).
    pub async fn wait_for_completion(&self) -> anyhow::Result<Vec<u8>> {
        let active = {
            let mut guard = self
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("Mutex do gravador envenenado"))?;
            guard.take()
        };
        let Some(active) = active else {
            anyhow::bail!("Nenhuma gravação ativa para aguardar");
        };
        active
            .rx
            .await
            .map_err(|_| anyhow::anyhow!("Thread de captura encerrou inesperadamente"))?
    }

    /// Versão síncrona (bloqueante) de `stop_async()`, retornando os bytes WAV 16 kHz mono 16-bit.
    pub fn stop(&self) -> anyhow::Result<Vec<u8>> {
        futures::executor::block_on(self.stop_async())
    }
}

fn run_capture_thread(
    tap_mode: bool,
    stop_flag: Arc<AtomicBool>,
    init_tx: std::sync::mpsc::SyncSender<anyhow::Result<()>>,
    done_tx: oneshot::Sender<anyhow::Result<Vec<u8>>>,
) {
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        let _ = init_tx.send(Err(anyhow::anyhow!(
            "Nenhum dispositivo de entrada (microfone) padrão disponível"
        )));
        return;
    };

    let supported_cfg = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = init_tx.send(Err(anyhow::anyhow!(
                "Não foi possível obter a configuração padrão do microfone: {e}"
            )));
            return;
        }
    };

    let sample_format = supported_cfg.sample_format();
    let config: StreamConfig = supported_cfg.into();
    let channels = config.channels;
    let src_rate = config.sample_rate;

    let samples_buf: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(
        (src_rate as usize) * 10,
    )));
    let detector = Arc::new(Mutex::new(SilenceDetector::default()));
    let auto_stopped = Arc::new(AtomicBool::new(false));

    let push_mono = {
        let samples_buf = Arc::clone(&samples_buf);
        let detector = Arc::clone(&detector);
        let auto_stopped = Arc::clone(&auto_stopped);
        move |mono: &[i16]| {
            if mono.is_empty() {
                return;
            }
            if let Ok(mut buf) = samples_buf.lock() {
                buf.extend_from_slice(mono);
            }
            if let Ok(mut det) = detector.lock() {
                let should_stop = det.feed_samples(mono, src_rate);
                if (tap_mode && should_stop) || det.total_elapsed() >= MAX_RECORD_DURATION {
                    auto_stopped.store(true, Ordering::SeqCst);
                }
            }
        }
    };

    let err_fn = |err| {
        tracing::warn!("[Microfone] Erro no fluxo de captura cpal: {err}");
    };

    let stream_res = match sample_format {
        SampleFormat::F32 => {
            let mut cb = push_mono;
            device.build_input_stream(
                config.clone(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_f32_to_mono_i16(data, channels);
                    cb(&mono);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let mut cb = push_mono;
            device.build_input_stream(
                config.clone(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_i16_to_mono(data, channels);
                    cb(&mono);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let mut cb = push_mono;
            device.build_input_stream(
                config.clone(),
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_u16_to_mono_i16(data, channels);
                    cb(&mono);
                },
                err_fn,
                None,
            )
        }
        other => {
            let _ = init_tx.send(Err(anyhow::anyhow!(
                "Formato de amostra do microfone não suportado: {other:?}"
            )));
            return;
        }
    };

    let stream = match stream_res {
        Ok(s) => s,
        Err(e) => {
            let _ = init_tx.send(Err(anyhow::anyhow!(
                "Falha ao construir fluxo de entrada do microfone: {e}"
            )));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = init_tx.send(Err(anyhow::anyhow!(
            "Falha ao iniciar captura do microfone: {e}"
        )));
        return;
    }

    let _ = init_tx.send(Ok(()));

    let started = Instant::now();
    while !stop_flag.load(Ordering::SeqCst)
        && !auto_stopped.load(Ordering::SeqCst)
        && started.elapsed() < MAX_RECORD_DURATION
    {
        std::thread::sleep(Duration::from_millis(20));
    }

    drop(stream);

    let captured = samples_buf
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default();

    let resampled = resample_mono(&captured, src_rate, TARGET_SAMPLE_RATE);
    let wav_result = encode_wav_16k_mono(&resampled);
    let _ = done_tx.send(wav_result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_encoding_produces_valid_16khz_mono_16bit_wav() {
        let input: Vec<i16> = (0..1600)
            .map(|i| ((i as f32 * 0.1).sin() * 12000.0) as i16)
            .collect();
        let wav_bytes = encode_wav_16k_mono(&input).expect("codificação WAV deve ter sucesso");

        assert!(wav_bytes.starts_with(b"RIFF"));
        assert_eq!(&wav_bytes[8..12], b"WAVE");

        let mut reader =
            hound::WavReader::new(Cursor::new(&wav_bytes)).expect("leitura WAV válida");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let decoded: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("amostras válidas");
        assert_eq!(decoded, input);
    }

    #[test]
    fn downmix_and_resample_convert_stereo_48k_to_mono_16k() {
        // 480 quadros estéreo (960 amostras) a 48 kHz = 10 ms -> 160 amostras mono a 16 kHz.
        let mut stereo = Vec::with_capacity(960);
        for _ in 0..480 {
            stereo.push(1000_i16);
            stereo.push(3000_i16);
        }
        let mono_48k = downmix_i16_to_mono(&stereo, 2);
        assert_eq!(mono_48k.len(), 480);
        assert!(mono_48k.iter().all(|&s| s == 2000));

        let mono_16k = resample_mono(&mono_48k, 48_000, TARGET_SAMPLE_RATE);
        assert_eq!(mono_16k.len(), 160);
        assert!(mono_16k.iter().all(|&s| s == 2000));
    }

    #[test]
    fn silence_detector_stops_after_2_5s_of_silence_following_speech() {
        let mut det = SilenceDetector::default();
        let step = Duration::from_millis(500);

        // Silêncio inicial antes de qualquer fala não deve disparar auto-stop de 2,5 s.
        for _ in 0..10 {
            assert!(!det.feed_rms(40.0, step));
        }
        assert!(!det.speech_detected());

        // Usuário começa a falar (RMS > 250).
        assert!(!det.feed_rms(1200.0, step));
        assert!(det.speech_detected());

        // Pausa curta (1,5 s < 2,5 s) seguida de mais fala não deve parar.
        assert!(!det.feed_rms(50.0, step));
        assert!(!det.feed_rms(50.0, step));
        assert!(!det.feed_rms(50.0, step));
        assert!(!det.feed_rms(900.0, step));
        assert_eq!(det.silence_elapsed(), Duration::ZERO);

        // Agora 2,5 s contínuos de silêncio após a fala devem encerrar no 5º bloco de 500 ms.
        assert!(!det.feed_rms(30.0, step)); // 0,5 s
        assert!(!det.feed_rms(30.0, step)); // 1,0 s
        assert!(!det.feed_rms(30.0, step)); // 1,5 s
        assert!(!det.feed_rms(30.0, step)); // 2,0 s
        assert!(det.feed_rms(30.0, step)); // 2,5 s -> auto-stop!
    }

    #[test]
    fn silence_detector_enforces_120s_cap_even_during_continuous_speech() {
        let mut det = SilenceDetector::default();
        let chunk = Duration::from_secs(10);

        for _ in 0..11 {
            assert!(!det.feed_rms(1500.0, chunk));
        }
        // Ao completar 120 s, encerra obrigatoriamente.
        assert!(det.feed_rms(1500.0, chunk));
        assert_eq!(det.total_elapsed(), Duration::from_secs(120));
    }
}
