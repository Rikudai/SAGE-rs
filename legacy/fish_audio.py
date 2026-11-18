"""
Cliente e Utilitário Fish Audio para o S.A.G.E..
Contém os IDs dos modelos de voz do S.A.G.E. (Homem de Ferro / UCM) na Fish Audio,
funções para busca/captura de modelos, síntese TTS (por reference_id ou áudio de referência)
e download de amostras de voz para uso no modelo Fish.
"""
import base64
import json
import os
import urllib.error
import urllib.parse
import urllib.request
from typing import Dict, Any, List, Optional

DEFAULT_FISH_API_KEY = os.environ.get("FISH_AUDIO_API_KEY", "")  # real key lives in config.json

# Catalogo legado das vozes do Jarvis na Fish Audio (mantido por compatibilidade).
JARVIS_FISH_MODELS: Dict[str, Dict[str, Any]] = {
    "jarvis_ucm_ptbr": {
        "id": "a5b93aeddcc948c19ea04f0afe9d178c",
        "title": "Jarvis (UCM) - Português Brasileiro",
        "language": "pt",
        "author": "Rogerio Geraldo",
        "likes": 806,
        "tasks": 50283,
        "description": "Voz principal do Jarvis (UCM) em Português Brasileiro (mais popular).",
    },
    "jarvis_homem_de_ferro_ptbr": {
        "id": "d9a100f6d45f43dea41bd0c160d7e578",
        "title": "Jarvis PT-BR 2 (Jarvis do filme Homem de Ferro)",
        "language": "pt",
        "author": "tijay70487",
        "likes": 25,
        "tasks": 3817,
        "description": "Jarvis do filme Homem de Ferro dublado em PT-BR.",
    },
    "jarvis_ptbr_v2": {
        "id": "82d13948027e4be69892dd3d0104e681",
        "title": "Jarvis (PT-BR)",
        "language": "pt",
        "author": "Victor santos",
        "likes": 91,
        "tasks": 11511,
        "description": "Voz alternativa do Jarvis em Português Brasileiro.",
    },
    "jarvis_portugues": {
        "id": "ec426c7ea3554caba8a5b077a4c701aa",
        "title": "Jarvis português",
        "language": "pt",
        "author": "tijay70487",
        "likes": 32,
        "tasks": 5158,
        "description": "Voz calma e profissional do Jarvis em Português.",
    },
    "jarvis_mcu_en": {
        "id": "612b878b113047d9a770c069c8b4fdfe",
        "title": "Jarvis (MCU) J.A.R.V.I.S",
        "language": "en",
        "author": "Fish Audio Community",
        "likes": 549,
        "tasks": 141866,
        "description": "Voz original em Inglês do J.A.R.V.I.S (Paul Bettany - MCU).",
    },
}

DEFAULT_JARVIS_MODEL_ID = JARVIS_FISH_MODELS["jarvis_ucm_ptbr"]["id"]
DEFAULT_FISH_MODEL_TIER = "s2.1-pro-free"


class FishAudioClient:
    """Cliente de integração com a API Fish Audio (https://api.fish.audio)."""

    BASE_URL = "https://api.fish.audio"

    def __init__(
        self,
        api_key: str = DEFAULT_FISH_API_KEY,
        model_id: str = DEFAULT_JARVIS_MODEL_ID,
        model_tier: str = DEFAULT_FISH_MODEL_TIER,
    ):
        self.api_key = api_key
        self.model_id = model_id
        self.model_tier = model_tier

    def _headers(
        self,
        content_type: Optional[str] = None,
        model_tier: Optional[str] = None,
    ) -> Dict[str, str]:
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "User-Agent": "SageAssistant/2.0",
        }
        tier = model_tier if model_tier is not None else self.model_tier
        if tier:
            headers["model"] = tier
        if content_type:
            headers["Content-Type"] = content_type
        return headers

    def search_sage_models(self, language: str = "pt", page_size: int = 10) -> List[Dict[str, Any]]:
        """Busca e captura os IDs de modelos do S.A.G.E. diretamente na API da Fish Audio."""
        params = urllib.parse.urlencode({
            "title": "sage",
            "language": language,
            "page_size": page_size,
        })
        url = f"{self.BASE_URL}/model?{params}"
        req = urllib.request.Request(url, headers=self._headers(model_tier=None))
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read().decode("utf-8"))

        results = []
        for item in data.get("items", []):
            results.append({
                "id": item.get("_id"),
                "title": item.get("title"),
                "description": item.get("description", ""),
                "languages": item.get("languages", []),
                "likes": item.get("like_count", 0),
                "tasks": item.get("task_count", 0),
                "author": item.get("author", {}).get("nickname", ""),
            })
        return results

    def get_model_details(self, model_id: Optional[str] = None) -> Dict[str, Any]:
        """Obtém os metadados completos e URLs de amostras de um modelo pelo ID."""
        target_id = model_id or self.model_id
        url = f"{self.BASE_URL}/model/{target_id}"
        req = urllib.request.Request(url, headers=self._headers(model_tier=None))
        with urllib.request.urlopen(req, timeout=15) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def _request_tts(self, payload: Dict[str, Any], tier: Optional[str]) -> bytes:
        body = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            f"{self.BASE_URL}/v1/tts",
            data=body,
            headers=self._headers(content_type="application/json", model_tier=tier),
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.read()

    def synthesize(
        self,
        text: str,
        output_path: str,
        reference_id: Optional[str] = None,
        reference_audio_path: Optional[str] = None,
        reference_text: Optional[str] = None,
        audio_format: str = "mp3",
        model_tier: Optional[str] = None,
    ) -> str:
        """
        Gera áudio TTS usando o modelo do S.A.G.E. na Fish Audio.
        Suporta tanto `reference_id` (ID do modelo na nuvem) quanto `reference_audio_path`
        (arquivo de áudio local para clonagem instantânea no modelo Fish).
        Automaticamente usa o tier gratuito ('s2.1-pro-free') ou o configurado, com fallback automático.
        """
        payload: Dict[str, Any] = {
            "text": text,
            "format": audio_format,
            "normalize": True,
            "latency": "normal",
        }

        if reference_audio_path and os.path.exists(reference_audio_path):
            with open(reference_audio_path, "rb") as f:
                audio_b64 = base64.b64encode(f.read()).decode("utf-8")
            payload["references"] = [
                {
                    "audio": audio_b64,
                    "text": reference_text or "",
                }
            ]
        else:
            payload["reference_id"] = reference_id or self.model_id

        chosen_tier = model_tier if model_tier is not None else self.model_tier
        if not chosen_tier:
            chosen_tier = DEFAULT_FISH_MODEL_TIER

        try:
            audio_bytes = self._request_tts(payload, tier=chosen_tier)
        except urllib.error.HTTPError as e:
            # Se der HTTP 402 e o tier tentado não era o free, tenta automaticamente s2.1-pro-free
            if e.code == 402 and chosen_tier != "s2.1-pro-free":
                try:
                    audio_bytes = self._request_tts(payload, tier="s2.1-pro-free")
                except urllib.error.HTTPError as e2:
                    raise RuntimeError(f"Erro HTTP {e2.code} na Fish Audio API: {e2.reason}") from e2
            else:
                err_body = ""
                try:
                    err_body = e.read().decode("utf-8", errors="ignore")
                except Exception:
                    pass
                if e.code == 402:
                    raise RuntimeError(
                        "Fish Audio API retornou HTTP 402 (Payment Required): "
                        "Créditos insuficientes na Fish Audio."
                    ) from e
                raise RuntimeError(f"Erro HTTP {e.code} na Fish Audio API: {err_body or e.reason}") from e

        os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
        with open(output_path, "wb") as f:
            f.write(audio_bytes)
        return output_path
