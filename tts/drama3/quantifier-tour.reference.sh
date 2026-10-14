#!/usr/bin/env bash
# GGUF servi = GGUF publié + les 87 matrices de la tour (tour-q8-tenseurs.txt) en Q8_0. Sans réseau.
# Vérifié le 14/10/2026 : sortie 23 631 152 352 octets, SHA-256 f3678f54c89769fd4c81c09814508f0553e76a496a2ca806aab9837b75d7ad59
# (identique avec un binaire compilé depuis un clone propre).
#   quantifier-tour.sh <audiocpp_gguf> <gguf d'origine> <sortie> [liste des tenseurs]
set -euo pipefail
GGUF_TOOL=$1 IN=$2 OUT=$3 LIST=${4:-$(dirname "$0")/tour-q8-tenseurs.txt}
ov=(); while read -r n; do [[ -n $n ]] && ov+=(--keep-type "$n=q8_0"); done < "$LIST"
(( ${#ov[@]} == 87 * 2 )) || { echo "liste inattendue : $(( ${#ov[@]} / 2 )) tenseurs" >&2; exit 1; }
"$GGUF_TOOL" --input "$IN" --output "$OUT" --type orig "${ov[@]}"
echo "f3678f54c89769fd4c81c09814508f0553e76a496a2ca806aab9837b75d7ad59  $OUT" | sha256sum -c -
