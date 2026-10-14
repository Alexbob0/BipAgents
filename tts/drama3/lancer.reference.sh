#!/usr/bin/env bash
# RÉFÉRENCE seulement (la mise en service passe par l'installateur BipAgents).
# Réseau podman interne : port publié sur 127.0.0.1:8099 uniquement, aucun accès sortant (testé le 14/10/2026).
# Dans le conteneur le serveur écoute 0.0.0.0:8099 ("host": "0.0.0.0" dans server.json).
set -euo pipefail
LAB=${LAB:-$HOME/tts-lab/drama3}
PASS=${PASS:-$HOME/Public/drama3-bips-passation}
IMG=docker.io/kyuz0/amd-strix-halo-toolboxes:vulkan-radv@sha256:e62a1631b48b3190c91e3013110e02a325c69eede4c6c7e60ca2146f531a3a7e
podman network exists bipagents-drama3 || podman network create --internal bipagents-drama3
exec podman run --rm --name drama3-bips --network bipagents-drama3 -p 127.0.0.1:8099:8099 \
  --userns keep-id --user "$(id -u):$(id -g)" --device /dev/dri --group-add keep-groups --security-opt label=disable \
  -v "$LAB/models:/srv/drama3/models:ro" -v "$LAB/audio.cpp:/srv/drama3/audio.cpp:ro" \
  -v "$PASS/voix:/srv/drama3/voix:ro" -v "$PASS/serveur:/srv/drama3/serveur:ro" \
  "$IMG" /srv/drama3/audio.cpp/build/lab-vulkan/bin/audiocpp_server --config /srv/drama3/serveur/server.json --no-ui
