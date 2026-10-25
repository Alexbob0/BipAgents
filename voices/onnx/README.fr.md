[🇬🇧 English](README.md) · 🇫🇷 Français

# Les voix des Bips sur le téléphone

Les apps parlent avec les voix des Bips sur le téléphone lui-même, sans le serveur : Pocket TTS 4.0.1 de Kyutai
converti en ONNX (le modèle de langue en 8 bits), exécuté par ONNX Runtime. Un paquet par langue (environ 105 à 125 Mo,
téléchargé au premier usage) et le décodeur audio qu'ils partagent (40 Mo).

| Fichier | Rôle |
|---|---|
| `build.py` | fabrique les paquets dans `build/voices/` (ignoré par git), vérifie les graphes contre pocket et fait parler un échantillon par langue dans `build/voices/samples/` |
| `wrappers.py` | les étapes de pocket réécrites pour l'export, chaque état en entrée et en sortie |
| `reference.py` | parle avec un paquet comme les apps : la boucle que leurs moteurs suivent |
| `textprep.py` | le texte tel que pocket le prépare, tokeniseur compris, sans pocket : ce que les apps recopient |
| `golden.py`, `golden/` | des exemples écrits par pocket lui-même ; `test_textprep.py` et les tests des apps s'y comparent |
| `tokenizers/` | le tokenizer.json de chaque langue (Kyutai, CC-BY-4.0) |
| `store_packs.py` | les paquets comme les stores les livrent : paquets hébergés par Apple (`build/voices-store/apple/*.aar`) et paquets à la demande de Google Play (`android/voicepacks/`) |

```bash
uv venv --python 3.12 voices/onnx/.venv && uv pip install --python voices/onnx/.venv/bin/python -r voices/onnx/requirements.txt
voices/onnx/.venv/bin/python voices/onnx/build.py
```

Les paquets ne sont jamais à une adresse publique. Les apps les reçoivent de l'App Store (envoyer les `.aar` sur App
Store Connect avec Transporter, puis les soumettre avec la version) et de Google Play (le script d'envoi de l'éditeur les met
dans le bundle) ; les serveurs de l'offre les reçoivent de leur guichet (copier `build/voices` dans son dossier `voices`).

```bash
voices/onnx/.venv/bin/python voices/onnx/store_packs.py          # les deux stores ; --apple ou --android pour un seul
```

Versions. Chaque manifeste dit de quel Pocket il vient (`engine` : « pocket-tts 4.0.1 ») et la version des paquets
(`package` : 2 depuis le 25/10/2026, Pocket 4.0.1 ; 1 ou absent : Pocket 3.3.0). Les apps envoient cette version au
bridge (`engine` dans `POST /v1/tts/prepare`) : un paquet 3.3.0 garde les noms réécrits dont ses voix ont besoin, un
paquet 4.x reçoit le texte tel quel ; une app qui ne l'envoie pas est supposée avoir les paquets 3.3.0. Chaque paquet
porte ses propres règles de texte (`text` : Pocket 4 change les tirets en virgules, PR #387) et le fondu d'entrée de
Pocket 4 sur les 5 premières ms de chaque morceau (`generation.fade_in_samples`) : un téléphone continue de parler avec
les paquets qu'il a jusqu'à ce que le store apporte les nouveaux. Le décodeur (`mimi-c54a51aa0964`) est le même pour les
deux : une fabrication garde un décodeur déjà présent dans `build/voices` aux mêmes poids, que téléphones et serveurs ne
retéléchargent pas. Les fichiers golden sont ceux de pocket 4.0.1, avec les règles (`text`) que lisent les tests des apps.

Mesuré sur un Mac M4, un seul fil (20 octobre 2026) : un paquet en 10 à 15 s ; la voix 6 à 8 fois plus vite que le
temps réel ; le graphe en 32 bits à 6e-6 de pocket, en 8 bits à 2 à 5 % sur une étape ; Whisper transcrit juste les
quatre échantillons. Pocket 4.0.1 (25/10/2026) : fr 120,6 Mo, en 130,0, de 107,8, es 107,5, décodeur 41,4 (inchangé) ;
32 bits à 3e-6 de pocket, 8 bits à 3 à 9 % sur une étape ; 5,5 à 7 fois le temps réel sur un fil ; chaque Bip dans
chaque langue face à PyTorch 4.0.1 : Whisper (medium) entend la même phrase d'essai des deux. Reste à mesurer sur un vrai iPhone et un Android de milieu de gamme.

Crédit, livré dans le manifeste de chaque paquet : Pocket TTS de Kyutai (https://github.com/kyutai-labs/pocket-tts),
poids sous CC-BY-4.0, convertis en ONNX et quantifiés en 8 bits. Les voix des Bips sont synthétiques (Qwen3-TTS,
Apache-2.0, puis clonées avec Pocket TTS, kyutai/pocket-tts, CC-BY-4.0). Les voix de Kyutai sous CC BY-NC ne sont jamais livrées.
