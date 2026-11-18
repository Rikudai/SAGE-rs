//! Teste real da voz: Fish Audio, Edge TTS, transcrição pelo Gemini e reprodução.
use rodio::Source;
use sage_core::{Config, EventBus, ModelHub};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load_default()?;
    let client = reqwest::Client::new();
    let frase = "Teste de voz do Seije. Se você está ouvindo isso, a voz está funcionando.";

    let key = sage_voice::tts::resolve_fish_api_key(&cfg).unwrap_or_default();
    match sage_voice::tts::synthesize_fish_audio(&client, frase, &key, &cfg.fish_audio_reference_id, &cfg.fish_audio_model).await {
        Ok(b) => println!("FISH ok: {} bytes", b.len()),
        Err(e) => println!("FISH falhou: {e:#}"),
    }
    let edge = sage_voice::tts::synthesize_edge_tts(frase, &cfg.voice_name, &cfg.voice_rate, &cfg.voice_pitch, &cfg.voice_volume).await;
    match &edge {
        Ok(b) => println!("EDGE ok: {} bytes", b.len()),
        Err(e) => println!("EDGE falhou: {e:#}"),
    }
    let mp3 = sage_voice::tts::synthesize_with_fallback(&client, frase, &cfg).await?;
    println!("FALLBACK ok: {} bytes", mp3.len());

    // Decodifica o MP3, reduz para 16 kHz mono e transcreve.
    let dec = rodio::Decoder::new(std::io::Cursor::new(mp3.clone()))?;
    let rate = dec.sample_rate().get() as f32;
    let ch = dec.channels().get() as usize;
    let all: Vec<f32> = dec.collect();
    let mono: Vec<f32> = all.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
    let step = rate / 16000.0;
    let mut out = vec![];
    let mut i = 0.0f32;
    while (i as usize) < mono.len() {
        out.push((mono[i as usize].clamp(-1.0, 1.0) * 32767.0) as i16);
        i += step;
    }
    let wav = sage_voice::recorder::encode_wav_16k_mono(&out)?;
    if let Ok(p) = std::env::var("VOICE_WAV_OUT") { std::fs::write(p, &wav)?; }
    let hub = sage_providers::Hub::new(&cfg)?;
    match hub.transcriber() {
        Some(t) => match t.transcribe(wav, "pt-BR").await {
            Ok(txt) => println!("TRANSCRIÇÃO: {txt}"),
            Err(e) => println!("TRANSCRIÇÃO falhou: {e:#}"),
        },
        None => println!("sem transcriber"),
    }

    let voice = sage_voice::Voice::new(&cfg, None::<Arc<dyn sage_core::Transcriber>>, EventBus::new());
    voice.speak("Pronto. A voz do Seije está funcionando.");
    tokio::time::sleep(std::time::Duration::from_secs(9)).await;
    println!("fala reproduzida (is_speaking agora = {})", voice.is_speaking());
    Ok(())
}
