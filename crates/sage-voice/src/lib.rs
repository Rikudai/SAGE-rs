//! TEMPORARY stub so the app compiles while the voice worker runs.
use sage_core::{Config, EventBus, Transcriber};
use std::sync::Arc;
pub struct Voice;
impl Voice {
    pub fn new(_c: &Config, _t: Option<Arc<dyn Transcriber>>, _b: EventBus) -> Self { Voice }
    pub fn speak(&self, _t: &str) {}
    pub fn stop_speaking(&self) {}
    pub fn cue(&self, _n: &str) {}
    pub fn start_listening(&self) {}
    pub async fn finish_listening(&self) -> anyhow::Result<String> { Ok(String::new()) }
}
