//! Teste real do cérebro: triagem (chat), ação única e uma orquestração pequena.
use sage_core::events::Command;
use sage_core::{Config, EventBus, ModelHub, SageEvent, Toolbox};
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load_default()?;
    let hub: Arc<dyn ModelHub> = Arc::new(sage_providers::Hub::new(&cfg)?);
    println!("provedores disponíveis: {:?}", hub.available());
    let tools: Arc<dyn Toolbox> = Arc::new(sage_tools::PcToolbox::new(&cfg));
    let bus = EventBus::new();
    let mut sub = bus.subscribe();
    let brain = sage_brain::Brain::new(Arc::new(RwLock::new(cfg)), hub, tools, bus.clone())?;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(brain.run(rx));

    let pedidos: Vec<String> = std::env::args().skip(1).collect();
    for p in pedidos {
        println!("\n=== PEDIDO: {p}");
        tx.send(Command::Ask { text: p, from_voice: false, images: vec![] }).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(420);
        loop {
            let ev = match tokio::time::timeout_at(deadline, sub.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => continue,
                Err(_) => {
                    println!("!! TIMEOUT");
                    break;
                }
            };
            match ev {
                SageEvent::Stream { .. } => {}
                SageEvent::ConfirmRequest { id, question } => {
                    println!("CONFIRM? {question} -> sim");
                    tx.send(Command::Confirm { id, yes: true }).await?;
                }
                SageEvent::Reply { spoken, full, emotion } => {
                    println!("REPLY [{emotion:?}] falado: {spoken}\nCOMPLETO: {}", full.chars().take(900).collect::<String>());
                    break;
                }
                other => println!("EV {}", serde_json::to_string(&other).unwrap_or_default().chars().take(260).collect::<String>()),
            }
        }
    }
    Ok(())
}
