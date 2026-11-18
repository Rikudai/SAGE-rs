//! Lista o catálogo de skills do PC e testa a busca: `cargo run -p sage-tools --example skills_check -- "pedido"`
fn main() {
    let cat = sage_tools::skills::SkillCatalog::new(sage_core::paths::root());
    let all = cat.list();
    let mut by: std::collections::BTreeMap<String, usize> = Default::default();
    for s in &all {
        *by.entry(s.source.clone()).or_default() += 1;
    }
    println!("{} skills: {by:?}", all.len());
    let q = std::env::args().nth(1).unwrap_or_else(|| "criar uma skill nova".into());
    for s in cat.search(&q, 5) {
        println!("- {} ({})", s.name, s.source);
    }
    let m = sage_tools::memory::MemoryStore::open(sage_core::paths::data().join("memory"));
    println!("memória: {} páginas", m.count());
    println!("{}", m.recall_block(&q));
}
