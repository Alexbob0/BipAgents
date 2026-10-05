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
