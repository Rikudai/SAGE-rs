"""Grava amostras da voz do Rimuru que o S.A.G.E. usa hoje na Fish Audio, para treinar o
modelo local (RVC) e não depender mais da API.

    .venv-voz\\Scripts\\python.exe tools\\voz_local\\gerar_amostras.py [--quantas 80]

Usa a chave e a voz (reference_id) do config/config.json. As amostras vão para
data/voz/amostras/*.wav (cerca de 8 a 12 minutos de fala, o bastante para um RVC).
Gasta créditos da Fish Audio: uma chamada por frase. Pula as que já existem.

Treino (uma vez, pode levar horas em CPU): no Applio (https://github.com/IAHispano/Applio),
aba Train, pasta do dataset = data/voz/amostras, nome "rimuru", 200 a 300 épocas. Copie o
rimuru.pth (e o .index) gerados para data/voz/rimuru/ e reinicie a voz local.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

RAIZ = Path(__file__).resolve().parents[2]
SAIDA = RAIZ / "data" / "voz" / "amostras"

FRASES = [
    "Olá! Eu sou o S.A.G.E., e hoje estou com o processamento afiado.",
    "Pode deixar comigo, isso aqui eu resolvo em um instante.",
    "Hmm, interessante. Deixa eu pensar um pouco antes de responder.",
    "Terminei a tarefa. Os detalhes estão no chat, se quiser conferir.",
    "Abri o navegador e já deixei a página pronta para você.",
    "O seu computador está tranquilo, com a memória quase toda livre.",
    "Encontrei três arquivos com esse nome. Qual deles você quer?",
    "Não consegui falar com o servidor agora. Vou tentar de novo daqui a pouco.",
    "Que tal fazermos uma pausa? Você está trabalhando há bastante tempo.",
    "Pronto, o volume está em cinquenta por cento.",
    "Bom dia! O céu está limpo e a temperatura deve chegar a vinte e oito graus.",
    "Boa noite. Se precisar de alguma coisa, é só me chamar.",
    "Isso é fácil. Um slime como eu já viu coisas bem mais complicadas.",
    "Calma, calma. Primeiro a gente entende o problema, depois resolve.",
    "Eu avisei que ia dar certo! Às vezes eu me impressiono comigo mesmo.",
    "Desculpa, não entendi direito. Pode repetir com outras palavras?",
    "A compilação terminou sem erros e todos os testes passaram.",
    "Achei uma atualização nova. Quer que eu instale agora ou mais tarde?",
    "Estou baixando o arquivo. Faltam uns dois minutos.",
    "O download terminou. Coloquei tudo na sua pasta de downloads.",
    "Uau, que pergunta boa. Vou pesquisar e já te conto.",
    "Segundo o que eu encontrei, a resposta mais provável é a segunda opção.",
    "Você tem uma reunião às três da tarde. Quer que eu te lembre antes?",
    "Lembrete: beber água também faz parte da produtividade.",
    "Eu salvei o seu trabalho antes de fechar o programa, não se preocupe.",
    "Hoje é quinta-feira, dezenove de novembro.",
    "São dez e meia da manhã.",
    "Tem certeza? Essa ação não pode ser desfeita.",
    "Feito! Agora está tudo organizado por data.",
    "Vou dividir essa tarefa em partes menores e cuidar de cada uma.",
    "Enquanto isso, posso continuar conversando com você normalmente.",
    "A tarefa em segundo plano ainda está rodando. Falta pouco.",
    "Hum, isso não parece certo. Deixa eu verificar de novo.",
    "Ah, entendi agora! Era só uma vírgula fora do lugar.",
    "Que tal ouvir uma música enquanto trabalha? Posso abrir o Spotify.",
    "Pausei a música para você ouvir melhor.",
    "Aumentei o brilho da tela.",
    "Desliguei o computador da tomada? Claro que não, eu não tenho mãos.",
    "Estou aqui, sempre de olho, mas sem atrapalhar.",
    "Se eu fosse um humano, já estaria cansado. Ainda bem que sou um slime.",
    "Pesquisei os preços e a loja mais barata fica a dois quilômetros daqui.",
    "A previsão diz que vai chover à tarde. Leve um guarda-chuva.",
    "Copiei o texto para a área de transferência.",
    "Traduzi a mensagem. Em português, ela diz: obrigado pela ajuda.",
    "Instalei o programa e criei um atalho na área de trabalho.",
    "Não encontrei nada com esse nome no seu computador.",
    "Quer que eu explique passo a passo ou prefere só o resultado?",
    "Resumindo: deu certo, mas dá para melhorar a velocidade.",
    "Ei! Você esqueceu de salvar aquele arquivo.",
    "Interessante… muito interessante. Isso muda tudo.",
    "Primeiro, abra as configurações. Depois, clique em voz.",
    "Número um, dois, três, quatro, cinco. Teste de microfone concluído.",
    "Eu me chamo S.A.G.E., que significa sistema avançado de gestão e execução.",
    "Gostei dessa ideia. Vamos tentar?",
    "Não se preocupe, eu cuido do resto.",
    "Isso vai levar um tempinho. Te aviso quando terminar.",
    "Prontinho, tudo atualizado e funcionando.",
    "Ops, algo deu errado. Mas eu já sei como consertar.",
    "Você quer a versão curta ou a versão completa da história?",
    "Era uma vez um slime que aprendeu a programar sozinho.",
    "Calculei aqui: dá cento e vinte e cinco reais no total.",
    "A bateria do controle está acabando. Melhor carregar.",
    "Encontrei o episódio. Vou abrir na televisão da sala.",
    "Legenda em português ativada.",
    "Hoje o dia foi produtivo. Você concluiu sete tarefas.",
    "Amanhã eu te lembro de terminar o projeto.",
    "Certo! Anotado na memória.",
    "Eu lembro disso. Você me contou na semana passada.",
    "Hum… deixa eu ver. Sim, está aqui.",
    "Ha ha, essa foi boa.",
    "Sério? Eu não esperava por essa.",
    "Muito bem! Você está pegando o jeito.",
    "Atenção: o disco C está quase cheio.",
    "Liberei quatro gigabytes apagando arquivos temporários.",
    "Fechei os programas que estavam travando o computador.",
    "A conexão com a internet voltou.",
    "Vou reiniciar em alguns segundos para aplicar a atualização.",
    "Voltei! Estou atualizado e rodando a versão nova.",
    "Até mais! Foi bom trabalhar com você hoje.",
]


def carregar_config():
    cfg = json.loads((RAIZ / "config" / "config.json").read_text(encoding="utf-8"))
    chave = (cfg.get("fish_audio_api_key") or "").strip()
    voz = (cfg.get("fish_audio_reference_id") or "").strip()
    modelo = (cfg.get("fish_audio_model") or "s2.1-pro-free").strip()
    if not chave or not voz:
        sys.exit("Falta a chave ou a voz da Fish Audio no config/config.json.")
    return chave, voz, modelo


def gerar(texto: str, chave: str, voz: str, modelo: str) -> bytes:
    corpo = json.dumps({"text": texto, "reference_id": voz, "format": "wav"}).encode("utf-8")
    req = urllib.request.Request(
        "https://api.fish.audio/v1/tts",
        data=corpo,
        headers={"Authorization": f"Bearer {chave}", "model": modelo, "Content-Type": "application/json", "User-Agent": "S.A.G.E./2.0"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--quantas", type=int, default=len(FRASES))
    args = ap.parse_args()
    chave, voz, modelo = carregar_config()
    SAIDA.mkdir(parents=True, exist_ok=True)
    feitas = 0
    for i, frase in enumerate(FRASES[: args.quantas], 1):
        destino = SAIDA / f"rimuru_{i:03d}.wav"
        if destino.exists() and destino.stat().st_size > 1000:
            continue
        try:
            destino.write_bytes(gerar(frase, chave, voz, modelo))
            (SAIDA / f"rimuru_{i:03d}.txt").write_text(frase, encoding="utf-8")
            feitas += 1
            print(f"[{i}/{args.quantas}] ok", flush=True)
        except urllib.error.HTTPError as e:
            print(f"[{i}] Fish Audio HTTP {e.code}: parei (sem créditos ou limite).", flush=True)
            if e.code in (401, 402, 403, 429):
                break
        except Exception as e:
            print(f"[{i}] erro: {e}", flush=True)
        time.sleep(0.4)
    total = len(list(SAIDA.glob("rimuru_*.wav")))
    print(f"Pronto: {feitas} novas, {total} amostras em {SAIDA}.")


if __name__ == "__main__":
    main()
