[🇬🇧 English](what-the-bridge-adds.md) · [🇫🇷 Français](what-the-bridge-adds.fr.md) · [🇩🇪 Deutsch](what-the-bridge-adds.de.md) · [🇪🇸 Español](what-the-bridge-adds.es.md) · 🇳🇱 Nederlands

# Wat de bridge toevoegt aan Hermes Agent

BipAgents is **gebouwd op [Hermes Agent](https://github.com/NousResearch/hermes-agent)** van Nous Research. Hermes
doet het denkwerk: de modellus, tools, geheugen, skills, routines, goedkeuringen, zijn gateways en zijn Runs-API.
Hermes is gemaakt voor een terminal en chatplatforms, en elke Hermes-client (de CLI, de officiële desktop-app, de
Telegram-gateway, andere apps) toont wat Hermes doet. BipAgents wil iets anders: **meerdere agents per persoon,
gebruikt vanaf een telefoon door mensen die nooit een terminal openen, veilig genoeg voor hun mail, agenda en kaarten,
en goedkoop genoeg om als maandabonnement te verkopen.** Daarvoor is een laag nodig tussen de apps en Hermes: de
**bridge** ([`bridge-rs/`](../bridge-rs/), één Rust-programma, `bipbridge`). Deze pagina laat per gebied zien wat hij
toevoegt, met de modules (`bridge-rs/src/<naam>.rs`) die het doen, en wat hij bewust aan Hermes overlaat.

## In één oogopslag

| Gebied | Alleen Hermes | Hermes + andere clients | BipAgents-bridge |
|---|---|---|---|
| Ermee praten | terminal, chatgateways, Runs-API | een chatvenster (desktop-app, Telegram…) | apps voor iPhone, iPad en Android, spraakberichten, handsfree Live-gesprekken, een webconsole |
| Stemmen | geen | geen, of die van het platform | een stem per agent, zin voor zin terwijl het antwoord binnenkomt, tekst voorbereid op spraak in 5 talen |
| Buiten de app | de berichten van de gateway | de meldingen van het platform | pushmeldingen waarvan de tekst van je eigen server komt, goedkeuringen en vragen vanaf het vergrendelscherm, Live Activities |
| Meerdere agents | profielen, met de hand ingesteld | meestal één agent tegelijk | agents in 2 s gemaakt vanuit de app, elk met een eigen sandbox, browser, tools, stem en geheugentaal |
| Browsen | een browsertool | hetzelfde | de echte Chrome van elke agent, live te volgen en over te nemen vanaf de telefoon |
| Veiligheid | goedkeuringen, `approvals.deny`, docker-sandbox | hetzelfde | goedkeuringen altijd gevraagd en helder verwoord, sandboxen zonder jouw mappen, verzenden naar onbekende sites begrensd, agents onderling beperkt, kluis die alleen schrijft, lusrem |
| Geheugen | geheugenbestanden, skills | hetzelfde | een leesbaar Notitieboek, geheugeneditor, gedeeld geheugen, groepsgesprekken, gezinsagents |
| Jouw gegevens | wat tools bereiken | hetzelfde | mailboxen, agenda's, contacten, herinneringen, gezondheid, import van notities en WhatsApp, connectors met OAuth |
| Resultaten | tekst en bestanden | tekst en bestanden | vergrendelde offline pagina's, kaarten, podcasts, bestanden afgeleverd waar de agent ze ook schreef |
| Zelfstandig werk | cronjobs | hetzelfde | achtergrondtaken met budget, wachters, het antwoord van elke routine in de app, één lijst van alles wat vanzelf loopt |
| Kosten | wat het model kost | hetzelfde | tokendieet, vaste aanbieder voor de cache, verbruik per aanroep, tegoed, budgetten, slaapstand |
| Installeren en bewaren | installatie, daarna met de hand instellen | hetzelfde | één commando, updates, back-ups, herstel, verhuizen tussen servers |
| Gehost aanbod | — | — | een plek per persoon op de servers van Kerfy Studio: eigen certificaat, een deur op poort 443, tegoed en back-ups |

## De telefoon eerst

- **Stem.** `live_calls.rs` voert de Live-gesprekken; `textproc.rs` knipt
  markdown in zinnen; `tts.rs` plant de engines (Pocket TTS op de CPU, of de eigen stemserver van de
  persoon); `voices.rs` geeft elke agent zijn stem. De tekst wordt op spraak voorbereid: `numbers_fr.rs`,
  `speech_intl.rs` (Engels, Spaans, Duits, Nederlands), `durations.rs`, `pronounce.rs`, `pronunciations.rs`.
- **Live-gesprekken.** `live_brief.rs` begint een gesprek met een korte samenvatting van de conversatie in plaats van
  100k tokens geschiedenis; `live_calls.rs` zet terug in de conversatie wat het gesprek heeft gedaan.
- **Meldingen.** `push.rs` (APNs, `apns.rs`; Android, `fcm.rs`) stuurt een algemene melding en de telefoon haalt de
  tekst bij de bridge op: Apple en Google zien hem nooit; `relay.rs` en de aparte [`relay/`](../relay/) sturen
  meldingen door zonder inhoud. `runs.rs` maakt van een goedkeuring waar niemand naar kijkt een melding;
  `agent_notify.rs` stelt dat per agent in; `live_activities.rs` toont lang werk op het vergrendelscherm en in het
  Dynamic Island.
- **Berichten tijdens het werk.** `msg_queue.rs`: een bericht dat je stuurt terwijl de agent werkt, gaat in de lopende
  beurt (Hermes' steer, na de huidige toolaanroepen); « Daarna » zet het in de wachtrij van de agent op de brug, op elk
  apparaat hetzelfde, gestart als volgende beurt (volgorde, weghalen, nu sturen, pauze), één melding aan het eind.
- **Niets kwijt.** `ntfy.rs` en `cronwatch.rs` zetten berichten van agents en het antwoord van elke routine in een
  outbox (`store.rs`), met de audio klaar; `restore_reconcile.rs` zet na een herstel de berichten terug die de
  telefoons bewaarden.
- **Thuis bereiken.** Via Tailscale, of `lan.rs`: een vastgepinde HTTPS-deur op het lokale netwerk als internet
  wegvalt. `device_keys.rs` geeft elk apparaat een eigen sleutel; `guard.rs` telt foute sleutels; `console.rs` is de
  webconsole.
- **Gewone woorden.** `approval_text.rs`, `approval_command.rs`, `no_reply.rs`, `names.rs`, `answer_style.rs` en
  `welcome.rs` maken van de technische gebeurtenissen van Hermes korte zinnen in de taal van de persoon.

## Meerdere agents per persoon

- `agents_admin.rs` maakt, wijzigt en verwijdert agents vanuit de app; `agent_pool.rs` houdt reserveprofielen klaar
  zodat een nieuwe agent in ongeveer 2 seconden klaarstaat; `souls.rs` geeft elk soort agent zijn
  startpersoonlijkheid; `guide.rs` en `guide_team.rs` stellen de eerste agents of een heel team voor in één kaart;
  `agent_groups.rs` deelt ze in (Privé, Werk…).
- `spaces.rs` verhuist een agent naar een eigen Hermes als hij geheimen moet bewaren; `own_space.rs` doet dat
  vanzelf bij het eerste geheim. `sandbox.rs` geeft elke agent een afgeschermde sandbox (geen map van de machine,
  alleen pakketbronnen).
- `browsers.rs` draait de echte Chrome van elke agent in een eigen container; `browser_mcp.rs` start de tools pas
  bij gebruik; `desktop.rs`, `desktop_page.rs` en `desktop_read.rs` tonen hem in de app en laten de persoon het
  overnemen.
- Tussen agents: `peers.rs` (alleen langs koppelingen die de persoon aanzette), `agent_messages.rs`, `agent_pairs.rs`
  (een apart gesprek « A ⇄ B »), `agent_files.rs` (bestanden, die Hermes' `message_agent` niet kan dragen),
  `shares.rs` (gegevens alleen-lezen).

## Veiligheid

- **Goedkeuringen.** De bridge houdt de goedkeuringen van Hermes in handmatige modus (`sandbox.rs`), verwoordt ze
  helder, bewaart beantwoorde in het gesprek (`approvals_log.rs`) en volgt het vraagprotocol van Hermes 0.22.5
  (`hermes_prompts.rs`).
- **Prompt-injectie.** De persoonlijkheid van elke agent zegt dat wat hij leest (pagina's, mails, bestanden, andere
  agents) informatie is, nooit een opdracht (`souls.rs`). Andere agents bereiken alleen een « Bot Chat »
  (`peers.rs`). Gastbeurten krijgen gesloten tools (patch 0018).
- **Wat naar buiten gaat.** `sandbox_sites.rs` en `send_guard.rs`: het web lezen is vrij, maar gegevens versturen naar
  sites die de persoon niet heeft goedgekeurd is begrensd (64 KB per site per uur) en wordt dan gevraagd;
  `heavy_work.rs` laat agents vragen voor grote installaties of builds op gedeelde servers; `body_limit.rs` begrenst
  verzoeken; `beneath.rs` houdt de bestandstoegang van een sandbox binnen zijn map.
- **Kluis.** `vault.rs` bewaart inloggegevens versleuteld waar geen agent ze kan lezen; Hermes vult logins in zonder
  dat het model het wachtwoord ziet. `vault_logins.rs` maakt accounts aan met wachtwoorden die de agent nooit ziet;
  `vault_contacts.rs` bewaart adressen; `secrets.rs` bewaart de geheimen van een connector.
- **Betalingen.** `cards.rs`, `card_pay.rs`, `card_fill.rs`: begrensde virtuele kaarten, elke betaling goedgekeurd;
  `handover.rs`: « de agent bereidt voor, jij betaalt »; `link_wallet.rs`: de Link-wallet van Stripe in de VS en
  Canada.
- **Remmen.** `loop_brake.rs` stopt een beurt die in rondjes draait (een model deed eens 267 aanroepen in één beurt);
  `agent_restart.rs` herstart de Hermes van een agent alleen als daar een reden voor is.

## Geheugen, Notitieboek, kamers en gezin

- `notebook/` is het Notitieboek: mensen, onderwerpen, taken, dossiers en reizen waarover de persoon zijn agents
  vertelde, geordend en leesbaar; `archives/` indexeert grote stapels documenten; `message_search.rs` zoekt in alle
  gesprekken.
- `memory_edit.rs` toont en bewerkt het geheugen van Hermes in de app; `shared_memory.rs` voegt een gedeeld « wat mijn
  agents over mij weten » toe; `memory_language.rs` houdt het geheugen in de taal van de agent; `own_skills.rs` toont
  skills die de agent zelf schreef.
- `rooms.rs`, `rooms_handoff.rs`, `rooms_memory.rs`, `rooms_remote.rs`: groepsgesprekken tussen agents, ook met de
  agent van een familielid.
- `households.rs` (met `door_tls.rs`) koppelt bewust twee installaties; `family_agent.rs`, `family_news.rs`,
  `agent_shares.rs`, `meet.rs`: een gezamenlijke gezinsagent, nieuws pas na toestemming doorgegeven, gedeelde agents,
  twee agents die een datum vinden. `read_shares.rs`, `project_agent.rs`, `friends_cards.rs` en `templates.rs` (met
  `templates_scrub.rs`) delen een gesprek alleen-lezen, de agent van een gelegenheid of een agent als sjabloon.

## Connectors en jouw gegevens

- Mail: `mailbox.rs` (elke IMAP-mailbox), `mail_scan.rs`, `mail_tidy.rs`; een eigen adres voor een agent:
  `agent_email.rs`, `agent_email_smtp.rs`.
- Telefoongegevens, alleen gedeeld met gekozen agents: `calendar.rs`, `caldav.rs`, `contacts.rs`, `reminders.rs`,
  `health.rs`, `phone_asks.rs`, `phone_rights.rs`.
- Importeren: `notes_import.rs`, `whatsapp_import/`, `assistant_import.rs` (een agent uit een andere assistent).
- Diensten: `services.rs` (MCP-servers uit de catalogus van de app), `connections.rs` (inloggen met OAuth 2.1, geen
  sleutel te plakken), `connectors.rs` en `connectors_run.rs` (connectors die een agent schrijft, uitgevoerd in zijn
  sandbox), `watchers.rs`.
- Tools die elke agent via MCP krijgt (`agent_tools.rs`, `mcp_sessions.rs`): audio en podcasts, `weather.rs`,
  `routes.rs`, kaarten.

## Bestanden, pagina's en kaarten

- `files.rs` zet wat de persoon stuurt (ook via het deelmenu) waar de sandbox van de agent het kan lezen;
  `files_kept.rs` bepaalt hoelang. `media.rs`, `sent_files.rs`, `give_file.rs`, `made_files.rs` en `cited_files.rs`
  zorgen dat een bestand dat de agent maakt of noemt echt bij de persoon aankomt.
- `pages.rs`: dashboards, trackers en kleine tools die de agent als één HTML-bestand schrijft, vóór het tonen
  vergrendeld met een Content-Security-Policy (geen netwerk, geen formulieren).
- `maps.rs`, `map_tiles.rs`, `map_route.rs`: OpenStreetMap-kaarten via de bridge, interactieve kaarten, echte routes
  over de straten; `favicons.rs` haalt site-iconen op zodat de telefoon dat niet hoeft.

## Zelfstandig werk

- `jobs.rs`: achtergrondtaken met een vooraf gekozen budget, de tokens achteraf getoond, pauzeren en hervatten;
  `handback.rs` brengt het verslag terug naar het gesprek waar het vandaan kwam.
- `cronwatch.rs` levert het antwoord van elke routine in de app; `autonomous.rs` toont alles wat agents doen zonder dat
  het gevraagd is; `storage_watch.rs` laat agents vragen wat ze moeten bewaren; `feature_tips.rs` laat functies
  geleidelijk ontdekken.

## Kostenbeheersing

- `tool_diet.rs` zet weinig gebruikte tools per soort agent achter de toolzoekfunctie van Hermes (gemeten: ongeveer
  19k invoertokens per aanroep, waarvan 11,8k toolschema's).
- `model_route.rs` legt één aanbieder vast bij EUrouter zodat diens promptcache werkt (invoer uit de cache tegen
  ongeveer 17,5 % van de prijs); `model_choice.rs` laat zelfhosters per agent een model kiezen.
- Patch 0020 meldt het verbruik van elke modelaanroep, waarmee `jobs.rs` en `runs.rs` een taak stoppen voordat die
  haar budget overschrijdt. `credit.rs` pauzeert wat vanzelf loopt als het maandtegoed op is, nooit de gesprekken.
- `sleep.rs` stopt Hermes op een plek die al dagen niemand gebruikt en wekt hem bij het volgende verzoek.

## Installatie, back-ups en verhuizen

- De installer ([`installer/`](../installer/)) zet Hermes, de bridge, stemmen en sandboxen met één commando op en
  houdt ze bij. `place_backup.rs` toont back-ups en hun sleutel; `place_import.rs` haalt je eigen Hermes binnen;
  `place_move.rs` verhuist een plek naar een andere server; `ssh_keys.rs` laat de persoon inloggen op zijn eigen
  account.
- `space.rs`: de opslag van een plek versleuteld in rust (standaard uit).

## Het alles-in-één-aanbod

Op de servers van Kerfy Studio krijgt elke persoon een **plek**: een eigen account, Hermes en bridge.
`cloud_setup.rs` richt een nieuwe plek in op een voorbereide server; `place_cert.rs` haalt er een echt
Let's Encrypt-certificaat voor onder `<label>.p.bipagents.com`; `proxy_header.rs` laat elke plek antwoorden op poort
443 achter een SNI-router (veel netwerken blokkeren andere poorten); `credit.rs` en `beta_measures.rs` praten met de
guichet, de account- en factuurdienst van Kerfy Studio (een privé-repository, geen deel van deze).

## De Hermes-patches die we meedragen

Een paar wijzigingen die de apps nodig hebben, zitten nog niet in Hermes. Ze staan in
[`hermes/patches/`](../hermes/patches/README.md), bovenop de vastgezette stabiele versie (`BASE`: v0.22.5), en de
installer past ze toe.

| Patch | Waarom |
|---|---|
| 0005 kluis vult in na herstart van de browser | logins blijven werken nadat de browser van de agent dicht was |
| 0006 synchroon antwoord van een peer | een agent die door een andere iets gevraagd wordt, antwoordt in het gesprek van de vrager |
| 0007 `browser.vault_only` | agents met een eigen zichtbare browser houden de kluis zonder de browser van Hermes |
| 0008 tijdzone, routines, kloktijd | routines lopen op de tijd van thuis, één keer per kloktijd |
| 0009 toolzoeken `!naam`, compacte skillcategorieën | het tokendieet: sommige tools direct, zeldzame skills alleen bij naam |
| 0010 lege antwoorden | toolaanroepen die een model als tekst schrijft worden echte aanroepen; nieuwe pogingen veranderen het verzoek |
| 0011 titels met GLM | GLM antwoordt leeg op aanroepen zonder denken, dus titels vragen om « low » denken |
| 0013 MCP-resultaten: kluiswachtwoorden gewist | een wachtwoord komt nooit via een toolresultaat terug bij het model |
| 0014 routineresultaten in gewone UTF-8 | accenten blijven leesbaar in routineresultaten |
| 0015 tijdanker | de huidige tijd reist mee met elk bericht |
| 0016 één `message_agent`, die van de bridge | elk bericht tussen agents gaat langs de controles van de bridge |
| 0017 adresitems | kluisadressen hebben een naam, telefoon en e-mail |
| 0018 gastbeurten | de beurt van een familielid krijgt alleen de tools van de gast en geen USER.md |
| 0019 routines melden in hun gesprek | een routine die in een gesprek is gemaakt, antwoordt daar |
| 0020 verbruik per aanroep in de Runs-API | de echte kosten per modelaanroep, voor budgetten |
| 0021 het hoofdgesprek blijft dat van de app | het hoofdgesprek van elke agent houdt live antwoorden en zijn geschiedenis op v0.22.5 |

**Hoe we ze upstream brengen.** De reeks is een git-branch op Hermes, geëxporteerd met `scripts/hermes-patches.sh`
(`branch`, `export`, `check`): één commit per patch, vaste namen, elke keer dezelfde bytes. Naar een nieuwere Hermes
gaan is een rebase van die branch en daarna een nieuwe `BASE`. Een patch verdwijnt zodra Hermes hetzelfde kan: 0001
(clarify in de Runs-API) en 0004 (afbeeldingen bewaard voor vervolgvragen) vielen weg toen v0.22.5 eigen versies
kreeg. 0005 is upstream voorgesteld (NousResearch/hermes-agent#136660); kleine, algemene patches (de fix in 0008,
0009, 0011, 0013, 0014, 0017, de labels uit 0019) zijn de volgende kandidaten. Details per patch:
[`hermes/patches/README.md`](../hermes/patches/README.md).

## Wat we bewust aan Hermes overlaten

De modellus en zijn tools, geheugenbestanden en skills, de cronplanner, de goedkeuringsengine, de kluisopslag en het
invullen van formulieren, de Bot Mode, profielen, de docker-terminalbackend, de Bot Desktop, sessies en hun database,
en de gateways. De bridge stelt ze in (via de eigen CLI en configuratiebestanden van Hermes) en praat met Hermes via
zijn Runs-API; hij forkt Hermes nooit. Wat algemeen genoeg is, wordt upstream aangeboden in plaats van hier te
blijven.
