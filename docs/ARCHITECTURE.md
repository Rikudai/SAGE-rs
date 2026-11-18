# S.A.G.E. em Rust — arquitetura

Reescrita do S.A.G.E. (C:\SAGE_backup, Python) só com o essencial: **cérebro orquestrador**,
**voz**, **controle do PC**, **slime na tela** e **interface nativa (egui)** de chat e configuração.
Sem web UI, sem Android, sem skills auto-escritas, sem estúdio de sprites.

## Crates

| Crate | Dono | O que faz | Depende de |
|---|---|---|---|
| `sage-core` | lead | contratos: `Config` (mesmo `config.json` antigo), `EventBus`/`SageEvent`/`Command`, traits `Llm`, `ModelHub`, `AgentCli`, `Transcriber`, `Toolbox`; persona e texto | — |
| `sage-providers` | worker | Gemini REST (SSE, imagens, transcrição de áudio), pool OpenAI-compatível (Groq, Cerebras, OpenRouter, Mistral, NVIDIA, SambaNova, Ollama), agentes CLI `claude`/`codex`/`agy`, `Hub: ModelHub`, cooldown de cota | core |
| `sage-tools` | worker | `PcToolbox: Toolbox` via Win32 (`windows` crate): PowerShell, mouse, teclado, janelas, captura de tela, arquivos, clipboard, abrir app/URL | core |
| `sage-voice` | worker | gravação push-to-talk (cpal), STT via `Transcriber`, TTS Fish Audio (+ Edge TTS fallback), playback (rodio), pausa de mídia (SMTC), sons de evento | core |
| `sage-avatar` | worker | slime em janela própria (viewport egui transparente, sempre no topo), animações PNG de `assets/sprites`, estados, arrastar, clones | core, egui |
| `sage-brain` | worker forte | o orquestrador (abaixo) | core |
| `app` (`sage.exe`) | lead | egui: chat + configurações + bandeja + atalhos; liga tudo | todos |

Regra: um crate só conhece `sage-core` e as próprias libs. A fiação fica no `app`.

## Cérebro orquestrador ("plan big, execute small")

Baseado no alethe-orchestrate (Kauã Miguel) e no cookbook *plan big, execute small* da Anthropic.
O **lead** (tier forte) só decompõe e sintetiza; **workers** baratos fazem a leitura e o trabalho.
Tudo configurável em `config/playbooks.yaml` (mesmo formato do Alethe).

### Fluxo de um pedido

1. **Triagem** (modelo `triage`, barato, JSON): `{"kind": "chat"|"action"|"task", "reply", "action", "action_params", "emotion"}`.
   - `chat`: responde direto (persona `chat`). Sem workers.
   - `action`: uma ferramenta do `Toolbox` e pronto (ex.: abrir o Spotify), depois uma frase curta.
   - `task`: vai para o orquestrador. Heurística de reforço: `text::looks_like_task` e `text::complexity >= 0.55`.
2. **Escolha do playbook** pelo lead: recebe `pickOrder`, resumos dos playbooks e o pedido; devolve
   `{"playbook", "why"}`. Pedido pequeno e claro → `direct` (o lead faz sozinho com ferramentas).
3. **Plano** pelo lead (JSON validado): para cada fase do playbook que não é `who: lead`, uma onda
   com unidades `{id, role, brief, kind: "pc"|"code"|"read"|"write-text", owned_paths, tier}`.
   O brief segue o contrato do worker (objetivo, pronto quando, escopo, checagens, proibido, retorno).
4. **Ondas**: cada fase vira uma onda de workers concorrentes (`maxConcurrent`, padrão 4).
   - Executor por `kind`: `pc`/`read`/`write-text` → **worker de API** com laço de ações JSON sobre o
     `Toolbox` (só ferramentas `mutates=false` quando `readOnly`); `code` → **agente CLI**
     (`claude`, `codex` ou `agy`, `AgentMode::ReadOnly|Write`, cwd = pasta do projeto do pedido).
   - **Tier → modelo**: `tiers[tier][provider]` + overrides do perfil ativo (`brain_profile`).
   - **Provedor**: primeiro de `providers`/`passive.prefer` que esteja disponível e com cota.
5. **Gate** de cada fase (texto `gate` do playbook) checado pelo lead ou por um revisor de outro
   provedor (tier standard): `ok` | `retry` (mesmo worker, com o feedback) | `escalate` (um tier acima,
   uma vez por unidade). Falha só escala por checagem que falhou, nunca pela palavra do worker.
6. **Síntese** pelo lead a partir só dos relatórios: `{"spoken": curto, "full": markdown, "emotion"}`.
7. **Relatório** (`data/runs/<run_id>/report.json` + `report.md`): playbook, porquê, ondas, cada
   worker (modelo, tier, duração, tokens, custo quando houver, status, marcação usado/descartado),
   trocas de provedor e o que ficou faltando. A UI mostra na aba *Execuções*.

### Cota e orçamento

- `QuotaLedger` em `data/quota.json`: por provedor, uso por janela (`5h`, `day`, `week`) observado
  em respostas (429/limites, `rate_limit_event` do Claude, mensagem "usage limit ... try again at"
  do Codex, `agy` "resets in"). Provedor ≥ `quota.criticalThreshold` → troca para o próximo no mesmo
  tier e emite `ProviderSwitch` ("Movendo os workers para o Groq: Gemini em 86%..."). ≥ `stopAt` →
  bloqueia. Todos baixos → `ConfirmRequest` com opções.
- `Budget` por execução: `maxCalls`, `maxWallSeconds`, `maxReportedTokens`, `maxReportedCostUsd`
  (de `budget` no plano, `brain_max_workers`/`brain_max_cost_usd` no config). Em `warnAt`% →
  `whenLow` (`finish-and-report` | `downgrade-tier` | `stop`).
- Princípios do playbook entram no prompt do lead (p.ex. "faça você mesmo quando o briefing custar mais").

### Eventos para a UI e o slime

`PlanReady`, `WorkerStart`/`WorkerTool`/`WorkerEnd` (o slime solta um clone por worker:
`spawn → work → done|fail → vanish`), `Gate`, `ProviderSwitch`, `RunReport`, `Stream` (raciocínio ao
vivo de lead e workers), `Reply`, `Emotion`, `Status`.

### API pública do `sage-brain`

```rust
pub struct Brain { /* ... */ }
impl Brain {
    pub fn new(cfg: Arc<RwLock<Config>>, hub: Arc<dyn ModelHub>, tools: Arc<dyn Toolbox>, bus: EventBus) -> anyhow::Result<Self>;
    /// Loop principal: consome comandos até o canal fechar.
    pub async fn run(self, rx: tokio::sync::mpsc::Receiver<Command>);
}
pub mod playbooks; // parse/resolve de config/playbooks.yaml
pub mod quota;     // QuotaLedger
pub mod report;    // RunReport
```

Histórico de conversa: últimas `context_turns` (8) trocas no prompt do triage/chat.

## Persona

Os blocos da persona são os do projeto antigo, copiados palavra por palavra (`sage-core::persona`),
e `data/persona.json` sobrescreve os editáveis. Nunca usar "Mestre"/"Chefe".

## Interface nativa (app)

- **Chat**: histórico com markdown simples, campo de texto, botão de microfone (segurar), status,
  painel lateral "Raciocínio ao vivo" (Stream por fonte) e "Workers" (onda atual).
- **Execuções**: lista de relatórios com custo/tokens por worker.
- **Configurações**: abas Geral, Voz, Modelos (chaves com campo de senha, modelos por papel),
  Cérebro (perfil, provedores, orçamento), Slime. Salva no `config/config.json`.
- Bandeja do sistema (abrir chat, mostrar/esconder slime, sair) e atalhos globais
  (`hotkey_listen` segurar para falar, `hotkey_panel` abre o chat).

## Build

Toolchain `stable-x86_64-pc-windows-gnu` (mingw em `C:\mingw64\bin` no PATH). `cargo build --release`.
`config/config.json` tem chaves: fica fora do git.

## Memória, skills e uso (2026-11-18)

- **Memória** (`crates/sage-tools/src/memory.rs`), no desenho do ai-memory: páginas
  markdown em `data/memory/wiki/<tipo>/` (fonte da verdade, editáveis à mão), captura de
  cada turno em `data/memory/raw/*.jsonl` e no diário do dia, fatos ditos explicitamente
  ("lembre que…", "eu prefiro…") salvos sem LLM, busca BM25 com bônus para duráveis e
  para o que é usado, recall injetado no prompt como dados, segredos removidos antes de
  gravar, diários antigos e nunca usados arquivados (nunca apagados). Ferramentas
  `memory_save`, `memory_search`, `memory_forget`. `data/memory/` fica fora do git.
- **Skills** (`crates/sage-tools/src/skills.rs`): catálogo de todos os `SKILL.md` do PC
  (SAGE, Claude Code, Codex, Antigravity, `~/.agents`) e ferramentas `skills_search`,
  `skill_read`, `skill_create`. O método do gerador é a skill `skills/skill-creator`
  (sickn33/antigravity-awesome-skills, Apache 2.0).
- **Transcrição**: Groq Whisper (`whisper-large-v3-turbo`, ~0,5 s) primeiro, Gemini de reserva.
- **Uso** (`app/src/usage.rs`): cota restante por provedor (Alethe `alethe-usage.json` +
  `data/quota.json`) e consumo por modelo das execuções.
- **Configurações ao vivo**: salvar troca hub, ferramentas, voz, atalhos e slime sem reiniciar;
  a aba Personalidade edita `data/persona.json`.
