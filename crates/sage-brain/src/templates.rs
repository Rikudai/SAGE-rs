//! Prompt texts the brain sends to the triage model and to the lead (pt-BR).

use sage_core::tools::ToolSpec;

use crate::jsonx::clip;
use crate::playbooks::{Playbook, Playbooks};
use crate::report::{WaveReport, status_label};
use crate::worker::tools_block;

/// Markers the prompts carry (also used by tests to route fake model replies).
pub const TRIAGE_MARKER: &str = "TRIAGEM DO PEDIDO";
pub const PICK_MARKER: &str = "ESCOLHA DO PLAYBOOK";
pub const PLAN_MARKER: &str = "FORMATO DO PLANO";
pub const SYNTH_MARKER: &str = "RELATÓRIOS DOS WORKERS";

/// Appended to the chat persona for the triage call.
pub fn triage_instructions(specs: &[ToolSpec], emotions: &[String]) -> String {
    format!(
        "{TRIAGE_MARKER}\n\
Classifique o último pedido do usuário e responda só com um objeto JSON:\n\
{{\"kind\": \"chat\" | \"action\" | \"task\", \"reply\": \"...\", \"action\": \"<nome da ferramenta>\" | \"none\", \
\"action_params\": {{...}}, \"emotion\": \"{}\"}}\n\
- chat: conversa, opinião ou pergunta que você responde agora; reply traz a resposta.\n\
- action: uma única ferramenta resolve (por exemplo abrir um programa); reply é uma frase curta do que vai ser feito.\n\
- task: precisa de vários passos, leitura de arquivos, código ou pesquisa; reply fica vazio.\n\n\
FERRAMENTAS:\n{}",
        emotions.join(" | "),
        tools_block(specs)
    )
}

/// The lead's system prompt: persona plus the playbook principles.
pub fn lead_system(persona_lead: &str, principles: &[String]) -> String {
    let mut s = persona_lead.trim().to_string();
    if !principles.is_empty() {
        s.push_str("\n\nPRINCÍPIOS DE ORQUESTRAÇÃO:\n");
        for p in principles {
            s.push_str("- ");
            s.push_str(p.trim());
            s.push('\n');
        }
    }
    s
}

pub fn pick_prompt(request: &str, pb: &Playbooks) -> String {
    format!(
        "{PICK_MARKER}\nPEDIDO DO USUÁRIO:\n{}\n\n{}\n\
Pedido pequeno e claro: use \"direct\" (você mesmo resolve com ferramentas).\n\
Responda só com JSON: {{\"playbook\": \"<id>\", \"why\": \"<uma frase em português>\", \"title\": \"<título curto da tarefa, 2 a 5 palavras, sem copiar o pedido>\"}}",
        request.trim(),
        pb.pick_menu()
    )
}

pub fn plan_prompt(request: &str, playbook_id: &str, playbook: &Playbook, max_units: usize) -> String {
    let mut s = format!(
        "{PLAN_MARKER}\nPEDIDO DO USUÁRIO:\n{}\n\nPLAYBOOK: {playbook_id} ({}): {}\n\nFASES QUE VIRAM ONDAS DE WORKERS (na ordem):\n",
        request.trim(),
        playbook.name,
        playbook.goal
    );
    for ph in playbook.phases.iter().filter(|p| p.is_wave()) {
        s.push_str(&ph.describe());
        s.push('\n');
    }
    let lead: Vec<&str> = playbook.phases.iter().filter(|p| p.is_lead()).map(|p| p.id.as_str()).collect();
    if !lead.is_empty() {
        s.push_str(&format!("\nFASES DO LEAD (você mesmo; não crie unidades): {}\n", lead.join(", ")));
    }
    let follow: Vec<&str> = playbook.phases.iter().filter(|p| p.is_follow_up()).map(|p| p.id.as_str()).collect();
    if !follow.is_empty() {
        s.push_str(&format!(
            "FASES DE ACOMPANHAMENTO (o orquestrador dispara quando um gate falha; não crie unidades): {}\n",
            follow.join(", ")
        ));
    }
    s.push_str(&format!(
        "\nREGRAS:\n\
- Divida por unidade independente e não mais fino; no máximo {max_units} unidades no total e 8 por onda.\n\
- Cada brief segue o contrato do worker: objetivo, pronto quando, escopo, checagens, proibido e o que retornar.\n\
- kind: \"pc\" (controlar o PC com ferramentas), \"code\" (agente de código numa pasta de projeto), \
\"read\" (ler ou pesquisar), \"write-text\" (escrever texto).\n\
- Em fases que alteram coisas, cada unidade tem owned_paths disjuntos.\n\
- Marque tags \"light\", \"deep\" ou \"risky\" quando couber; unidade risky nunca roda no tier cheap.\n\
- Se o pedido for pequeno e claro, devolva \"waves\": [] e você mesmo resolve com ferramentas.\n\n\
Responda só com JSON:\n\
{{\"summary\": \"...\", \"cwd\": \"pasta do projeto ou null\", \"waves\": [{{\"phase\": \"<id da fase>\", \"units\": \
[{{\"id\": \"curto-sem-espacos\", \"role\": \"...\", \"brief\": \"...\", \"kind\": \"read\", \"owned_paths\": [], \
\"tier\": \"cheap\" | \"standard\" | \"strong\", \"tags\": []}}]}}], \"budget\": {{\"maxCalls\": 40}}}}"
    ));
    s
}

pub fn synthesis_prompt(request: &str, playbook: &str, waves: &[WaveReport], missing: &[String], emotions: &[String]) -> String {
    let mut s = format!("PEDIDO DO USUÁRIO:\n{}\n\nPLAYBOOK: {playbook}\n\n{SYNTH_MARKER} (responda só a partir deles):\n", request.trim());
    for w in waves.iter().flat_map(|w| w.workers.iter()) {
        let mark = if w.used { "usado" } else { "descartado" };
        s.push_str(&format!(
            "\n### {} (fase {}, {}, {mark})\nResumo: {}\n{}\n",
            w.id,
            w.phase,
            status_label(w.status),
            w.summary.trim(),
            clip(w.output.trim(), 3000)
        ));
    }
    if !missing.is_empty() {
        s.push_str(&format!("\nFICOU FALTANDO: {}\n", missing.join(", ")));
    }
    s.push_str(&format!(
        "\nResponda só com JSON: {{\"spoken\": \"uma ou duas frases para falar em voz alta\", \
\"full\": \"resposta completa em markdown\", \"emotion\": \"{}\"}}",
        emotions.join(" | ")
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_carry_their_markers_only() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/playbooks.yaml");
        let pb = Playbooks::load(&p).unwrap();
        let pick = pick_prompt("faça x", &pb);
        assert!(pick.contains(PICK_MARKER) && !pick.contains(PLAN_MARKER));
        let plan = plan_prompt("faça x", "complete", pb.playbook("complete").unwrap(), 6);
        assert!(plan.contains(PLAN_MARKER) && plan.contains("`build`") && !plan.contains("`fix`"));
        assert!(plan.contains("no máximo 6 unidades"));
        let synth = synthesis_prompt("faça x", "complete", &[], &["a".to_string()], &["feliz".to_string()]);
        assert!(synth.contains(SYNTH_MARKER) && synth.contains("FICOU FALTANDO: a"));
        let tri = triage_instructions(&[], &["feliz".to_string()]);
        assert!(tri.contains(TRIAGE_MARKER) && tri.contains("feliz"));
        assert!(lead_system("lead", &["p1".to_string()]).contains("- p1"));
    }
}
