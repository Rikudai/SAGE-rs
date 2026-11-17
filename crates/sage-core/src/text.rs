//! Text helpers ported from `sage/text_processing.py` and `sage/intent_guard.py`.
//!
//! PORT TODO (worker "core-text"): implement with the `regex` crate (no
//! lookaround: rewrite those patterns by hand), keep the signatures.

/// "S.A.G.E." / "SAGE" / "Sage" -> "Seije" for TTS, leaving file extensions alone.
pub fn pronounce_name(_text: &str) -> String {
    unimplemented!()
}

/// Strip markdown/code/URLs/paths for speech and keep at most `max_sentences`.
pub fn sanitize_for_speech(_text: &str, _max_sentences: usize) -> String {
    unimplemented!()
}

/// Remove a leading wake word ("sage", "ei sage", ...) and misheard variants.
pub fn strip_wake_word(_text: &str, _wake_words: &[String]) -> String {
    unimplemented!()
}

/// Fix common STT mistakes ("Beach Hub" -> "GitHub").
pub fn fix_transcription(_text: &str) -> String {
    unimplemented!()
}

/// Heuristic: does this look like a task (vs chit-chat)?
pub fn looks_like_task(_text: &str) -> bool {
    unimplemented!()
}

/// Does the request refer to what is on screen?
pub fn needs_screen_context(_text: &str) -> bool {
    unimplemented!()
}

/// Reply only promises work ("vou fazer", "deixa comigo") without doing it.
pub fn is_empty_promise(_text: &str) -> bool {
    unimplemented!()
}

/// Request complexity in 0.0..=1.0 (port of model_registry heuristic).
pub fn complexity(_text: &str) -> f32 {
    unimplemented!()
}
