//! Teste real da transcrição: `cargo run -p sage-providers --example transcribe_check -- arquivo.wav`
use sage_core::ModelHub;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("caminho do .wav");
    let wav = std::fs::read(&path)?;
    let cfg = sage_core::Config::load_default()?;
    let http = reqwest::Client::new();
    if let Some(w) = sage_providers::whisper::WhisperTranscriber::from_config(&cfg, http) {
        let t = Instant::now();
        let r = sage_core::Transcriber::transcribe(&w, wav.clone(), "pt-BR").await;
        println!("GROQ WHISPER {:.2}s: {:?}", t.elapsed().as_secs_f32(), r.map_err(|e| e.to_string()));
    }
    let hub = sage_providers::Hub::new(&cfg)?;
    let t = Instant::now();
    let r = hub.transcriber().expect("transcritor").transcribe(wav, "pt-BR").await;
    println!("CADEIA {:.2}s: {:?}", t.elapsed().as_secs_f32(), r.map_err(|e| e.to_string()));
    Ok(())
}
