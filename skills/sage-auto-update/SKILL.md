---
name: sage-auto-update
description: Atualiza o próprio S.A.G.E. pelo GitHub (Rikudai/SAGE-rs), reinicia na versão nova e publica as mudanças locais. Use em "se atualize", "tem atualização?", "o que tem de novo em você", "reinicie", "suba seu repositório para o GitHub", "publique suas mudanças" e para preparar o S.A.G.E. em outro PC.
---

# Auto-atualização do S.A.G.E.

Porte do `sage/self_update.py` do S.A.G.E. antigo (Python) para o app em Rust. O lugar é a pasta
raiz do S.A.G.E. (a que tem `config/`, `assets/` e `.git`, normalmente `C:\SAGE`).

## Ferramentas nativas (prefira estas)

| Pedido | Ferramenta |
| --- | --- |
| "tem atualização?" | `sage_update` com `{"apply": false}`: só verifica e lista as novidades |
| "se atualize" | `sage_update` (padrão `apply: true`): `git pull --ff-only`, `cargo build --release -p sage`, instala o `sage.exe` novo; se não compilar, volta ao commit anterior (`git reset --keep`) |
| "reinicie" / depois de atualizar | avise em uma frase e chame `sage_restart` (reabre em ~6 s) |
| "suba para o GitHub", "publique" | `sage_publish` com uma mensagem curta em português: commit + push e o `sage.exe` no release `sage-latest` |
| "o que mudou em você?" | as novidades da última atualização já entram no contexto (`data/ultima_atualizacao.json`); explique sem jargão de git |

Fluxo completo de "se atualize": `sage_update` → se instalou, diga o que veio de novo → `sage_restart`.
Fluxo de "suba e desligue": `sage_publish` → `power` com `{"action": "shutdown"}`, sem perguntar.

## Ao abrir

`update_on_start` no `config/config.json` (também em Configurações → Geral):

- `auto` (padrão): 20 s depois de abrir, procura no GitHub; se houver novidade, baixa, compila e
  instala, e avisa no chat que vale no próximo reinício.
- `check`: só avisa que há novidade.
- `off`: não procura. A variável de ambiente `SAGE_SKIP_UPDATE=1` também desliga.

Com commits locais que não estão no GitHub, ele não aplica sozinho: publique antes (`sage_publish`).
O registro fica em `logs/atualizacao.log`.

## Sem Rust no PC

Sem `cargo`, o `sage_update` baixa o `sage.exe` pronto do release `sage-latest` com o GitHub CLI
(`gh release download sage-latest -p sage.exe`). O `sage_publish` mantém esse release em dia.

## Chaves de API e outro PC

As chaves NUNCA vão em texto puro para o GitHub (`config/config.json` está no `.gitignore` e o
`sage_publish` recusa qualquer diff com chave). Para levar as chaves, use o cofre criptografado
(AES-256 + HMAC, senha do usuário):

```powershell
# neste PC: cria config\secrets.enc (pode ir para o GitHub)
powershell -ExecutionPolicy Bypass -File scripts\chaves-exportar.ps1
# no PC novo: clona, baixa o exe, restaura as chaves e cria o atalho de inicialização
powershell -ExecutionPolicy Bypass -File scripts\instalar.ps1
```

Nunca escreva a senha do cofre em arquivos, logs ou commits.

## Sem fazer

- Não use `git push --force`, `git reset --hard` nem apague `data/` (memória e humor do usuário).
- Não edite `config/config.json` para publicar; ele fica só neste PC.
