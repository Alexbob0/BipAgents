# Serveur Pocket TTS — les voix des Bips

Petit serveur HTTP autour de [Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) : 100 M de paramètres,
**sur CPU, sans GPU**, en français, anglais, espagnol et allemand. Le bridge (`bridge/`) l'appelle pour toutes les
voix `pocket:…` ; l'app ne lui parle jamais directement.

| Machine | Premier son | Vitesse | Mémoire (4 langues) |
|---|---|---|---|
| Mac mini / MacBook Air **Apple M4** (mesuré, 16 Go) | ≈ 50 ms | ≈ 6,5× le temps réel | ≈ 0,6 Go résident |
| VPS x86, 2 à 4 vCPU dédiés (estimé) | quelques centaines de ms | > temps réel | idem, moins avec `--quantize` |

Une génération à la fois (Pocket n'est pas prévu pour le parallèle et occupe le CPU) : le bridge met de toute
façon ses demandes en file. Prévoir 2 cœurs libres pour lui à côté de Hermes.

## Installation

Python ≥ 3.10. Le dépôt cloné dans `~/BipAgents` (les voix sont dans `~/BipAgents/voices`).

```bash
cd ~/BipAgents/pocket
python3 -m venv .venv            # ou : uv venv --python 3.12 .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python pocket_server.py --voices ../voices --languages fr,en,es,de
# premier lancement : téléchargement des modèles depuis Hugging Face (≈ 440 Mo par langue, une fois)
curl -s http://127.0.0.1:8098/health
```

Service au démarrage : `launchd/io.github.bipagents.pocket.plist` (macOS) ou `systemd/pocket-tts.service` (Linux),
cf. [`docs/self-hosting.md`](../docs/self-hosting.md). Côté bridge : `[pocket] url = "http://127.0.0.1:8098"`.

Options : `--languages fr,en` (ne charger que certaines langues), `--port 8098`, `--host 127.0.0.1` (laisser en
local : seul le bridge l'appelle), `--quantize` (poids int8 : moins de mémoire, ≈ 25 % plus rapide sur x86).

Les voix des Bips sont des états pré-calculés (`voices/<langue>/<voix>.safetensors`) : les lire ne demande que le
modèle public `kyutai/pocket-tts-without-voice-cloning`, téléchargé automatiquement. Le modèle de clonage (accès
Hugging Face à demander) ne sert qu'à **fabriquer** de nouvelles voix, cf. [`voices/README.md`](../voices/README.md).

## API (celle de Kyutai, utilisée par le bridge)

- `GET /health` → `{"status": "ok", "languages": ["fr", "en", …], "voices": {"fr": ["loutre", …], …}}`
- `POST /v1/audio/speech` `{"input": "…", "voice": "loutre", "response_format": "wav" | "mp3"}` → le fichier entier
- `POST /v1/audio/stream` `{"input": "…", "voice": "en/loutre"}` → PCM16 mono 24 kHz brut, morceau par morceau ;
  la génération s'arrête si le client coupe (interruption à la voix, réponse annulée)

`voice` : `loutre` (français) ou `<code>/loutre` avec `en`, `es`, `de`. Voix inconnue → 404 ; le bridge répond alors
une erreur et l'app lit le texte avec la voix de l'iPhone. Le texte arrive déjà préparé par le bridge (nombres en
lettres, symboles, abréviations).

Tests (sans modèle ni téléchargement) : `python -m pytest pocket/tests` (avec `fastapi`, `httpx` et `pytest`, par
exemple depuis le venv du bridge).

Licences : code MIT ; poids Pocket TTS CC-BY-4.0 (Kyutai) ; voix des Bips : cf. `voices/`.
