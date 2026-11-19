"""Voz local do S.A.G.E.: TTS sem API externa, servido em http://127.0.0.1:5123.

Motores (o primeiro instalado vale; dá para fixar em data/voz/config.json):
  * kokoro: Kokoro-82M em ONNX (int8), voz pt-BR feminina "pf_dora". Rápido em CPU.
  * piper:  Piper, voz pt_BR "faber". O mais rápido em CPU (voz masculina).
Timbre do Rimuru (opcional): se houver um modelo RVC em data/voz/rimuru/*.pth (e o pacote
rvc-python instalado), a fala sai do motor acima e passa pelo RVC, ficando com a voz dele.

  POST /tts    {"text": "...", "speed": 1.0}  ->  audio/wav
  GET  /saude                                  ->  {"motor": ..., "rvc": ...}

Instalação: scripts/voz-local-instalar.ps1. O S.A.G.E. abre este servidor sozinho quando o
provedor de voz é "local".
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

RAIZ = Path(__file__).resolve().parents[2]
VOZ = RAIZ / "data" / "voz"
MODELOS = VOZ / "modelos"
RIMURU = VOZ / "rimuru"
PORTA = int(os.environ.get("SAGE_VOZ_PORTA", "5123"))


def ler_config() -> dict:
    try:
        return json.loads((VOZ / "config.json").read_text(encoding="utf-8"))
    except Exception:
        return {}


def wav_bytes(amostras, taxa: int) -> bytes:
    """float32 [-1, 1] (numpy) -> WAV PCM 16 bits mono."""
    import numpy as np

    pcm = (np.clip(amostras, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(taxa)
        w.writeframes(pcm)
    return buf.getvalue()


class Kokoro:
    nome = "kokoro"

    def __init__(self, cfg: dict):
        from kokoro_onnx import Kokoro as K

        modelo = MODELOS / "kokoro-v1.0.int8.onnx"
        if not modelo.exists():
            modelo = MODELOS / "kokoro-v1.0.onnx"
        self.k = K(str(modelo), str(MODELOS / "voices-v1.0.bin"))
        self.voz = cfg.get("voz_kokoro", "pf_dora")

    def falar(self, texto: str, velocidade: float) -> bytes:
        amostras, taxa = self.k.create(texto, voice=self.voz, speed=velocidade, lang="pt-br")
        return wav_bytes(amostras, taxa)


class Piper:
    nome = "piper"

    def __init__(self, cfg: dict):
        from piper import PiperVoice

        nome = cfg.get("voz_piper", "pt_BR-faber-medium")
        self.v = PiperVoice.load(str(MODELOS / f"{nome}.onnx"))

    def falar(self, texto: str, velocidade: float) -> bytes:
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            if hasattr(self.v, "synthesize_wav"):  # piper-tts >= 1.3
                from piper import SynthesisConfig

                self.v.synthesize_wav(texto, w, syn_config=SynthesisConfig(length_scale=1.0 / max(velocidade, 0.3)))
            else:
                self.v.synthesize(texto, w, length_scale=1.0 / max(velocidade, 0.3))
        return buf.getvalue()


class Rvc:
    """Converte o timbre para o do modelo RVC em data/voz/rimuru."""

    def __init__(self, cfg: dict):
        modelos = sorted(RIMURU.glob("*.pth"))
        if not modelos:
            raise FileNotFoundError("sem modelo .pth em data/voz/rimuru")
        from rvc_python.infer import RVCInference

        self.r = RVCInference(device="cpu:0")
        indices = sorted(RIMURU.glob("*.index"))
        self.r.load_model(str(modelos[0]), index_path=str(indices[0]) if indices else "")
        self.r.set_params(f0up_key=int(cfg.get("rvc_tom", 0)), f0method=cfg.get("rvc_f0", "rmvpe"), index_rate=0.6)
        self.modelo = modelos[0].name

    def converter(self, wav: bytes) -> bytes:
        with tempfile.TemporaryDirectory() as d:
            ent, sai = Path(d) / "ent.wav", Path(d) / "sai.wav"
            ent.write_bytes(wav)
            self.r.infer_file(str(ent), str(sai))
            return sai.read_bytes()


def carregar():
    cfg = ler_config()
    ordem = [cfg["motor"]] if cfg.get("motor") in ("kokoro", "piper") else ["kokoro", "piper"]
    motor, erros = None, []
    for nome in ordem:
        try:
            motor = Kokoro(cfg) if nome == "kokoro" else Piper(cfg)
            break
        except Exception as e:  # motor não instalado ou sem modelo
            erros.append(f"{nome}: {e}")
    if motor is None:
        sys.exit("Nenhum motor de voz local instalado (" + "; ".join(erros) + "). Rode scripts/voz-local-instalar.ps1.")
    rvc = None
    if cfg.get("rvc", True):
        try:
            rvc = Rvc(cfg)
        except Exception as e:
            print(f"[voz] sem RVC: {e}", flush=True)
    return motor, rvc


MOTOR, RVC = carregar()
TRAVA = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def _json(self, code: int, obj: dict):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/saude"):
            self._json(200, {"motor": MOTOR.nome, "rvc": getattr(RVC, "modelo", None)})
        else:
            self._json(404, {"erro": "rota desconhecida"})

    def do_POST(self):
        if not self.path.startswith("/tts"):
            return self._json(404, {"erro": "rota desconhecida"})
        try:
            n = int(self.headers.get("Content-Length", "0"))
            bruto = self.rfile.read(n) or b"{}"
            try:
                pedido = json.loads(bruto.decode("utf-8"))
            except UnicodeDecodeError:  # cliente que mandou em cp1252
                pedido = json.loads(bruto.decode("cp1252"))
            texto = str(pedido.get("text", "")).strip()
            if not texto:
                return self._json(400, {"erro": "texto vazio"})
            with TRAVA:
                wav = MOTOR.falar(texto, float(pedido.get("speed", 1.0)))
                if RVC is not None and pedido.get("rvc", True):
                    wav = RVC.converter(wav)
        except Exception as e:
            return self._json(500, {"erro": str(e)})
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(len(wav)))
        self.end_headers()
        self.wfile.write(wav)


if __name__ == "__main__":
    print(f"[voz] motor {MOTOR.nome}, RVC {getattr(RVC, 'modelo', 'não')}, porta {PORTA}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORTA), Handler).serve_forever()
