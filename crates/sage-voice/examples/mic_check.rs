//! Abre o microfone padrão, grava 3 s e informa amostras e nível (sem precisar falar).
fn main() -> anyhow::Result<()> {
    let r = sage_voice::Recorder::new();
    r.start()?;
    std::thread::sleep(std::time::Duration::from_secs(3));
    let wav = r.stop()?;
    println!("MIC ok: WAV de {} bytes (~{:.1} s a 16 kHz)", wav.len(), (wav.len().saturating_sub(44)) as f32 / 32000.0);
    Ok(())
}
