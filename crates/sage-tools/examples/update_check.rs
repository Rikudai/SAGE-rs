//! Teste ao vivo da auto-atualização: `cargo run -p sage-tools --example update_check [publish]`.
//! Sem argumento só verifica o GitHub; com `publish`, publica (commit + push + release).

#[tokio::main]
async fn main() {
    let root = sage_core::paths::root().to_path_buf();
    println!("raiz: {}", root.display());
    match sage_tools::update::status(&root, 60).await {
        Ok(st) => println!(
            "ramo {} @ {} | {} novidade(s) no GitHub | {} commit(s) locais a publicar",
            st.branch,
            &st.head[..st.head.len().min(8)],
            st.incoming.len(),
            st.ahead
        ),
        Err(e) => println!("status falhou: {e}"),
    }
    if std::env::args().any(|a| a == "publish") {
        match sage_tools::update::publish(&root, "", true).await {
            Ok(m) => println!("{m}"),
            Err(e) => println!("publicar falhou: {e}"),
        }
    }
}
