[🇬🇧 English](hermes-clarify-api.md) · 🇫🇷 Français

# `clarify` dans l'api_server Hermes (Runs API) — spécification

Hermes 0.21 expose l'outil `clarify` aux agents de l'api_server mais ne le branche pas : `_create_agent()`
ne passe aucun `clarify_callback` à `AIAgent`, et l'outil répond « Clarify tool is not available in this
execution context ». Ce document décrit le branchement attendu par BipAgents. Il est **symétrique des
approbations** (`_make_approval_notify`, `POST /v1/runs/{id}/approval`).

## Côté Hermes (correctif)

1. Dans `gateway/platforms/api_server_runs.py`, pour chaque run :
   - poser `agent.clarify_callback` (ou le passer à `_create_agent()`) ;
   - le callback enregistre la question dans `tools/clarify_gateway.py` (`register`), émet l'événement
     `clarify.request` (ci-dessous), passe le run au statut `waiting_for_input`, puis attend la réponse
     (`wait_for_response`) jusqu'à `agent.clarify_timeout` ;
   - réponse reçue → statut `running`, événement `clarify.responded`, l'outil renvoie la réponse à l'agent ;
   - délai écoulé ou run arrêté → événement `clarify.cancelled`, l'outil renvoie « pas de réponse »
     (même comportement qu'une plateforme de messagerie sans réponse).
2. Nouvelle route `POST /v1/runs/{run_id}/clarify` qui appelle `resolve_gateway_clarify`.
3. `clarify_timeout` des profils servis à l'app : **30 min** (l'utilisateur répond souvent depuis une
   notification, plus tard).
4. Capacité annoncée dans `/v1/capabilities` : `"run_clarify": true`.

## Événements SSE (`GET /v1/runs/{run_id}/events`)

```
event: clarify.request
data: {"type": "clarify.request", "run_id": "run_…", "request_id": "clr_…",
       "questions": [{"id": "q1", "question": "Quel train ?", "choices": ["9h — 19 €", "14h — 35 €"],
                      "allow_other": true}]}
```

- Une question simple de `clarify(question, choices)` donne un tableau `questions` d'un élément (`id` libre,
  par exemple `"q1"`). Le mode « plusieurs questions » de l'outil donne un élément par question.
- `choices` peut être vide (question ouverte) ; `allow_other` dit si une réponse libre est acceptée.

```
event: clarify.responded
data: {"type": "clarify.responded", "run_id": "run_…", "request_id": "clr_…", "answers": {"q1": "9h — 19 €"}}

event: clarify.cancelled
data: {"type": "clarify.cancelled", "run_id": "run_…", "request_id": "clr_…", "reason": "timeout" | "stopped"}
```

## Réponse (`POST /v1/runs/{run_id}/clarify`)

```json
{"request_id": "clr_…", "answers": {"q1": "9h — 19 €"}}
```

- La valeur est le texte d'un choix, ou une réponse libre si `allow_other`.
- `{"request_id": "clr_…", "cancel": true}` annule (l'agent reçoit « pas de réponse »).
- Réponses : `200 {"resolved": 1}` ; `404` run ou question inconnus / expirés ; `409` déjà répondue.

## Côté BipAgents

- L'app affiche une carte « Question » avec un bouton par choix (un clic suffit pour une question
  simple ; « Autre… » ouvre un champ si `allow_other`), et répond par la route ci-dessus.
- Le bridge relaie ces événements comme les autres et, quand l'app ne suit pas le run, envoie une
  notification « <agent> a une question » qui rouvre la conversation.

## Depuis Hermes 0.22.5 (clarify d'upstream)

Depuis le 2026-11-01 BipAgents tourne sur Hermes 0.22.5, dont l'« autorité de session » mène chaque tour de l'API et
a son propre clarify : le patch 0001 est abandonné, et ce qui précède reste comme sa trace. Le contrat désormais :

- `clarify.request` porte une seule question au premier niveau (`question`, `choices`, `multi_select`) avec son
  `prompt_id` et son `execution_generation`. Un lot de l'outil arrive une question après l'autre. Le bridge ajoute
  `request_id` (le `prompt_id`), l'identifiant que lisent les apps et les notifications.
- La réponse est `POST /v1/runs/{run_id}/clarify` avec exactement
  `{"request_id": "<prompt_id>", "execution_generation": <n>, "answer": "<texte>"}`.
- Pas d'abandon. Une question passée part avec le mot « passer » de Hermes, `"\u0000skipped"` (`SKIPPED` de
  `tools/clarify_gateway.py`, ce que devient le « skip » tapé dans une messagerie) : l'agent la lit comme passée et la
  question suivante du lot arrive quand même. Une `answer` **vide** est lue comme pas de réponse du tout
  (`[user did not respond within 60m]`, tout le lot s'arrête) : les apps envoient toujours une réponse vide pour
  « Passer » et le bridge la change en ce mot (mesuré sur la v0.22.5, 2026-11-04).
- Le premier choix arrive avec l'étiquette anglaise « (Recommended) » de Hermes ; le bridge l'enlève avant les apps
  (Hermes l'enlève de toute façon de la réponse).
- `clarify.settled` et `approval.settled` terminent une invite (répondue, expirée ou disparue) ; ils ne portent pas
  les réponses.
- Les approbations prennent exactement `{"request_id": "<prompt_id>", "execution_generation": <n>, "choice": …}`.
  Sans la génération (le bouton d'une notification), les apps et le bridge lisent d'abord l'invite dans
  `GET /v1/runs/{id}` (`pending_controls`).
- Le `clarify_batch` d'upstream (toutes les questions d'un coup) n'est offert en 0.22.5 qu'aux affichages locaux,
  pas sur la Runs API.
