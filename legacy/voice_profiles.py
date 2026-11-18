"""
Voice profiles for S.A.G.E.

Each profile bundles the cloud voice (Fish Audio) with an offline Edge-TTS
fallback that keeps the same timbre, so speech quality degrades gracefully
when the network or API credits are unavailable.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Optional


@dataclass(frozen=True)
class VoiceProfile:
    key: str
    label: str
    description: str
    provider: str
    fish_model_id: str
    fish_tier: str
    edge_voice: str
    rate: str
    pitch: str
    volume: str = "+0%"


# IDs verified against the Fish Audio catalogue.
PROFILES: Dict[str, VoiceProfile] = {
    "rimuru": VoiceProfile(
        key="rimuru",
        label="Rimuru",
        description="Voz jovem, leve e animada (Rimuru Tempest PT-BR).",
        provider="fish_audio",
        fish_model_id="9dfb8ae2b936473d892167cc3f3a7233",
        fish_tier="s2.1-pro-free",
        edge_voice="pt-BR-ThalitaMultilingualNeural",
        rate="+6%",
        pitch="+10Hz",
    ),
    "sabio": VoiceProfile(
        key="sabio",
        label="Grande Sábio",
        description="Voz calma e precisa para relatórios e análises.",
        provider="fish_audio",
        fish_model_id="e4b076f01a4442eeb91d026393bc21ce",
        fish_tier="s2.1-pro-free",
        edge_voice="pt-BR-FranciscaNeural",
        rate="+3%",
        pitch="+2Hz",
    ),
    "classica": VoiceProfile(
        key="classica",
        label="Clássica (S.A.G.E.)",
        description="Voz formal e grave, no estilo do S.A.G.E. original.",
        provider="fish_audio",
        fish_model_id="a5b93aeddcc948c19ea04f0afe9d178c",
        fish_tier="s2.1-pro-free",
        edge_voice="pt-BR-AntonioNeural",
        rate="+0%",
        pitch="-2Hz",
    ),
}

DEFAULT_PROFILE = "rimuru"


def get_profile(key: Optional[str]) -> VoiceProfile:
    """Returns a known profile, falling back to the default."""
    return PROFILES.get((key or "").lower().strip(), PROFILES[DEFAULT_PROFILE])


def profile_keys():
    return list(PROFILES.keys())
