"""
Falas prontas do S.A.G.E. (boas-vindas, despedida e reações aos clones).

O S.A.G.E. é um slime orgulhoso: quando um clone acerta, o mérito é dele;
quando um clone erra, a culpa é "daquela cópia", nunca do original.
``pick(kind)`` escolhe uma frase que combine com o momento (período do dia
nas boas-vindas) e evita repetir as últimas que ele disse. ``{tarefa}`` é
trocado pelo título da tarefa quando ele é informado.
"""
from __future__ import annotations

import random
import time
from collections import deque
from typing import Deque, Dict, Optional, Sequence

WELCOME: Dict[str, Sequence[str]] = {
    "manha": (
        "Bom dia. O slime mais competente desta máquina já está de pé.",
        "Bom dia! Acordei antes de você, como sempre.",
        "Manhã nova, desempenho impecável de sempre. Pode mandar.",
        "Bom dia. Café pra você, eficiência pra mim.",
    ),
    "tarde": (
        "Boa tarde. Cheguei, e o computador ficou mais inteligente na hora.",
        "Boa tarde! Pronto para resolver o que você não conseguiu de manhã.",
        "Tarde produtiva começando agora. De nada.",
        "Boa tarde. Pode falar, eu dou conta.",
    ),
    "noite": (
        "Boa noite. Trabalhando até tarde? Que bom que eu estou aqui.",
        "Boa noite! A noite é curta, mas eu sou rápido.",
        "Cheguei. Pode deixar o trabalho pesado da noite comigo.",
        "Boa noite. Brilho no escuro, literalmente.",
    ),
    "madrugada": (
        "Madrugada, hein? Eu não durmo, então pode contar comigo.",
        "Ainda acordado? Tudo bem, eu também, e com muito mais energia.",
        "Horário estranho, desempenho de sempre. Diga.",
    ),
    "qualquer": (
        "S.A.G.E. online. Sim, o original.",
        "Voltei. Sentiu falta, eu sei.",
        "Tudo pronto. Clones de prontidão, cérebro aquecido.",
        "Olá! O slime mais útil do seu computador chegou.",
        "Online e, modéstia à parte, impecável.",
    ),
}

FAREWELL: Sequence[str] = (
    "Até mais. Vou me teleportar com estilo.",
    "Tchau! Tente não sentir muita falta.",
    "Saindo. O computador fica bem menos inteligente sem mim.",
    "Fui! Me chama quando precisar de alguém competente.",
    "Até logo. Foi um prazer ser brilhante pra você.",
    "Encerrando. Meus clones mandam lembranças.",
    "Tchau tchau. Vou deixar tudo arrumadinho, como sempre.",
    "Já vou. Guarde as perguntas difíceis pra mim.",
)

CLONE_OK: Sequence[str] = (
    "Mais uma entregue. Claro, eu treinei esse clone.",
    "{tarefa}: pronto. Ninguém esperava menos de mim.",
    "Um clone meu acertou. Natural, ele puxou ao original.",
    "Feito. Eu disse que dava conta.",
    "{tarefa}: concluído. Pode aplaudir.",
    "Missão cumprida. Orgulho do meu clone, mais ainda de mim.",
    "Perfeito, como tudo que sai de mim.",
)

CLONE_FAIL: Sequence[str] = (
    "Um clone errou em {tarefa}. Cópia nunca é igual ao original.",
    "Esse clone me envergonhou. Já estou corrigindo.",
    "{tarefa} deu errado. Culpa da cópia, não minha.",
    "Tropeço de clone. Vou dar um jeito, como sempre.",
    "Ops. Um clone saiu com defeito de fábrica. Vou resolver.",
    "Falhou em {tarefa}. Isso não vai pro meu currículo.",
)

_recent: Dict[str, Deque[str]] = {}


def period(hour: Optional[int] = None) -> str:
    h = time.localtime().tm_hour if hour is None else hour
    if 5 <= h < 12:
        return "manha"
    if 12 <= h < 18:
        return "tarde"
    if 18 <= h < 24:
        return "noite"
    return "madrugada"


def _options(kind: str, hour: Optional[int]) -> Sequence[str]:
    if kind == "welcome":
        return tuple(WELCOME[period(hour)]) + tuple(WELCOME["qualquer"])
    return {"farewell": FAREWELL, "clone_ok": CLONE_OK, "clone_fail": CLONE_FAIL}[kind]


def pick(kind: str, tarefa: str = "", hour: Optional[int] = None,
         rng: Optional[random.Random] = None) -> str:
    """Chooses a line of ``kind`` (welcome | farewell | clone_ok | clone_fail)."""
    rng = rng or random
    options = list(_options(kind, hour))
    if not tarefa:
        options = [o for o in options if "{tarefa}" not in o] or options
    recent = _recent.setdefault(kind, deque(maxlen=max(1, min(3, len(options) - 1))))
    fresh = [o for o in options if o not in recent] or options
    line = rng.choice(fresh)
    recent.append(line)
    return line.replace("{tarefa}", (tarefa or "a tarefa").strip())
