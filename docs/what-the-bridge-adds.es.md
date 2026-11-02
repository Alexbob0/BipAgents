[🇬🇧 English](what-the-bridge-adds.md) · [🇫🇷 Français](what-the-bridge-adds.fr.md) · [🇩🇪 Deutsch](what-the-bridge-adds.de.md) · 🇪🇸 Español · [🇳🇱 Nederlands](what-the-bridge-adds.nl.md)

# Lo que el bridge añade a Hermes Agent

BipAgents está **construido sobre [Hermes Agent](https://github.com/NousResearch/hermes-agent)** de Nous Research.
Hermes se encarga de pensar: el bucle del modelo, las herramientas, la memoria, las habilidades, las rutinas, las
aprobaciones, sus pasarelas y su API Runs. Hermes está pensado para un terminal y plataformas de chat, y cada cliente
de Hermes (la CLI, la app de escritorio oficial, la pasarela de Telegram, otras apps) muestra lo que hace Hermes.
BipAgents busca otra cosa: **varios agentes por persona, usados desde el móvil por gente que nunca abre un terminal,
lo bastante seguros para manejar su correo, su agenda y sus tarjetas, y lo bastante baratos para venderse como
suscripción mensual.** Eso exige una capa entre las apps y Hermes: el **bridge** ([`bridge-rs/`](../bridge-rs/), un
solo programa en Rust, `bipbridge`). Esta página recoge lo que añade, área por área, con los módulos
(`bridge-rs/src/<nombre>.rs`) que lo hacen, y lo que deja a propósito a Hermes.

## De un vistazo

| Área | Hermes solo | Hermes + otros clientes | Bridge de BipAgents |
|---|---|---|---|
| Hablarle | terminal, pasarelas de chat, API Runs | una ventana de chat (app de escritorio, Telegram…) | apps para iPhone, iPad y Android, notas de voz, llamadas Live manos libres, una consola web |
| Voces | ninguna | ninguna, o la de la plataforma | una voz por agente, frase a frase mientras llega la respuesta, texto preparado para hablar en 5 idiomas |
| Fuera de la app | los mensajes de la pasarela | las notificaciones de la plataforma | notificaciones cuyo texto viene de tu servidor, aprobaciones y preguntas desde la pantalla de bloqueo, Live Activities |
| Varios agentes | perfiles, configurados a mano | normalmente un agente a la vez | agentes creados en 2 s desde la app, cada uno con su sandbox, su navegador, sus herramientas, su voz y el idioma de su memoria |
| Navegar | una herramienta de navegador | igual | el Chrome real de cada agente, visto en directo y tomado desde el móvil |
| Seguridad | aprobaciones, `approvals.deny`, sandbox docker | igual | aprobaciones siempre pedidas y dichas con claridad, sandboxes sin tus carpetas, envíos a sitios desconocidos limitados, agentes entre sí limitados, caja fuerte de solo escritura, freno de bucles |
| Memoria | archivos de memoria, habilidades | igual | un Cuaderno legible, editor de memoria, memoria común, conversaciones de grupo, agentes de la familia |
| Tus datos | lo que alcanzan las herramientas | igual | buzones, calendarios, contactos, recordatorios, salud, importación de notas y WhatsApp, conectores con OAuth |
| Resultados | texto y archivos | texto y archivos | páginas bloqueadas sin conexión, mapas, pódcasts, archivos entregados dondequiera que el agente los escribiera |
| Trabajo por su cuenta | tareas cron | igual | tareas en segundo plano con presupuesto, vigilantes, la respuesta de cada rutina en la app, una lista de todo lo que funciona solo |
| Coste | lo que cueste el modelo | igual | dieta de tokens, proveedor fijo para la caché, consumo por llamada, crédito, presupuestos, reposo |
| Instalar y conservar | instalación y luego configuración a mano | igual | un comando, actualizaciones, copias de seguridad, restauraciones, mudanzas entre servidores |
| Oferta alojada | — | — | un espacio por persona en los servidores de Kerfy Studio: su certificado, una puerta en el puerto 443, crédito y copias |

## El móvil primero

- **Voz.** `voice_ws.rs` sigue una ejecución de Hermes y envía una frase sintetizada cada vez; `textproc.rs` corta el
  markdown en frases a medida que llega; `tts.rs` organiza los motores (Pocket TTS en CPU, o el servidor de voz propio
  de la persona); `voices.rs` da a cada agente su voz. El texto se prepara para hablar: `numbers_fr.rs`,
  `speech_intl.rs` (inglés, español, alemán, neerlandés), `durations.rs`, `pronounce.rs`, `pronunciations.rs`.
- **Llamadas Live.** `live_brief.rs` empieza una llamada con un breve resumen de la conversación en lugar de 100k
  tokens de historial; `live_calls.rs` devuelve a la conversación lo que hizo la llamada.
- **Notificaciones.** `push.rs` (APNs, `apns.rs`; Android, `fcm.rs`) manda un aviso genérico y el móvil recoge el
  texto del bridge: Apple y Google nunca lo ven; `relay.rs` y el [`relay/`](../relay/) aparte reenvían avisos sin
  contenido. `runs.rs` convierte en notificación una aprobación que nadie mira; `agent_notify.rs` lo ajusta por
  agente; `live_activities.rs` muestra un trabajo largo en la pantalla de bloqueo y la Dynamic Island.
- **Mensajes durante el trabajo.** `msg_queue.rs`: un mensaje enviado mientras el agente trabaja entra en el turno en
  curso (el steer de Hermes, tras sus llamadas a herramientas del momento); « Después » lo guarda en la cola del agente, en
  el puente, igual en todos los dispositivos, lanzado como siguiente turno al terminar (orden, quitar, enviar ahora,
  pausa), una sola notificación al final.
- **Nada se pierde.** `ntfy.rs` y `cronwatch.rs` guardan los mensajes de los agentes y la respuesta de cada rutina en
  una bandeja de salida (`store.rs`), con el audio listo; `restore_reconcile.rs` recupera tras una restauración los
  mensajes que guardaron los móviles.
- **Llegar a casa.** Por Tailscale, o `lan.rs`: una puerta HTTPS fijada en la red local cuando falla Internet.
  `device_keys.rs` da a cada dispositivo su clave; `guard.rs` cuenta las claves erróneas; `console.rs` es la consola
  web.
- **Palabras sencillas.** `approval_text.rs`, `approval_command.rs`, `no_reply.rs`, `names.rs`, `answer_style.rs` y
  `welcome.rs` convierten los eventos técnicos de Hermes en frases cortas en el idioma de la persona.

## Varios agentes por persona

- `agents_admin.rs` crea, cambia y elimina agentes desde la app; `agent_pool.rs` guarda perfiles de reserva para que un
  agente nuevo esté listo en unos 2 segundos; `souls.rs` da a cada tipo de agente su personalidad inicial; `guide.rs`
  y `guide_team.rs` proponen los primeros agentes o un equipo entero en una tarjeta; `agent_groups.rs` los ordena
  (Personal, Trabajo…).
- `spaces.rs` lleva un agente a su propio Hermes cuando debe guardar secretos; `own_space.rs` lo hace solo con el
  primer secreto. `sandbox.rs` da a cada agente un sandbox aislado (ninguna carpeta de la máquina, solo los
  repositorios de paquetes).
- `browsers.rs` ejecuta el Chrome real de cada agente en su propio contenedor; `browser_mcp.rs` arranca sus
  herramientas cuando se usan; `desktop.rs`, `desktop_page.rs` y `desktop_read.rs` lo muestran en la app y dejan que
  la persona tome el control.
- Entre agentes: `peers.rs` (solo por los enlaces que la persona activó), `agent_messages.rs`, `agent_pairs.rs` (una
  conversación « A ⇄ B » aparte), `agent_files.rs` (archivos, que el `message_agent` de Hermes no puede llevar),
  `shares.rs` (datos de solo lectura).

## Seguridad

- **Aprobaciones.** El bridge mantiene las aprobaciones de Hermes en modo manual (`sandbox.rs`), las redacta con
  claridad, guarda las ya respondidas en la conversación (`approvals_log.rs`) y sigue el protocolo de peticiones de
  Hermes 0.22.5 (`hermes_prompts.rs`).
- **Inyección de instrucciones.** La personalidad de cada agente dice que lo que lee (páginas, correos, archivos,
  otros agentes) es información, nunca órdenes (`souls.rs`). Los demás agentes solo llegan a un « Bot Chat »
  (`peers.rs`). Los turnos de invitados tienen herramientas cerradas (parche 0018).
- **Lo que sale.** `sandbox_sites.rs` y `send_guard.rs`: leer la web es libre, pero enviar datos a sitios que la
  persona no ha validado tiene un límite (64 KB por sitio y hora) y luego se pregunta; `heavy_work.rs` hace que los
  agentes pregunten antes de grandes instalaciones o compilaciones en servidores compartidos; `body_limit.rs` limita
  las peticiones; `beneath.rs` mantiene el acceso a archivos de un sandbox dentro de su carpeta.
- **Caja fuerte.** `vault.rs` guarda las credenciales cifradas donde ningún agente puede leerlas; Hermes rellena los
  inicios de sesión sin que el modelo vea la contraseña. `vault_logins.rs` crea cuentas con contraseñas que el agente
  nunca ve; `vault_contacts.rs` guarda direcciones; `secrets.rs` guarda los secretos de un conector.
- **Pagos.** `cards.rs`, `card_pay.rs`, `card_fill.rs`: tarjetas virtuales con límite, cada pago aprobado;
  `handover.rs`: « el agente prepara, tú pagas »; `link_wallet.rs`: la cartera Link de Stripe en EE. UU. y Canadá.
- **Frenos.** `loop_brake.rs` detiene un turno que da vueltas (un modelo hizo una vez 267 llamadas en un solo turno);
  `agent_restart.rs` reinicia el Hermes de un agente solo cuando hay motivo.

## Memoria, Cuaderno, salas y familia

- `notebook/` es el Cuaderno: personas, temas, tareas, expedientes y viajes que la persona contó a sus agentes,
  ordenados y legibles; `archives/` indexa grandes montones de documentos; `message_search.rs` busca en todas las
  conversaciones.
- `memory_edit.rs` muestra y edita la memoria de Hermes en la app; `shared_memory.rs` añade un « lo que mis agentes
  saben de mí » común; `memory_language.rs` mantiene la memoria en el idioma del agente; `own_skills.rs` lista las
  habilidades que el agente escribió él mismo.
- `rooms.rs`, `rooms_handoff.rs`, `rooms_memory.rs`, `rooms_remote.rs`: conversaciones de grupo entre agentes,
  incluido el agente de un familiar.
- `households.rs` (con `door_tls.rs`) une dos instalaciones a propósito; `family_agent.rs`, `family_news.rs`,
  `agent_shares.rs`, `meet.rs`: un agente común a la familia, noticias transmitidas solo tras aprobación, agentes
  compartidos, dos agentes que encuentran una fecha. `read_shares.rs`, `project_agent.rs`, `friends_cards.rs` y
  `templates.rs` (con `templates_scrub.rs`) comparten una conversación en solo lectura, el agente de una ocasión o un
  agente como plantilla.

## Conectores y tus datos

- Correo: `mailbox.rs` (cualquier buzón IMAP), `mail_scan.rs`, `mail_tidy.rs`; una dirección propia para un agente:
  `agent_email.rs`, `agent_email_smtp.rs`.
- Datos del móvil, compartidos solo con los agentes elegidos: `calendar.rs`, `caldav.rs`, `contacts.rs`,
  `reminders.rs`, `health.rs`, `phone_asks.rs`, `phone_rights.rs`.
- Importaciones: `notes_import.rs`, `whatsapp_import/`, `assistant_import.rs` (un agente traído de otro asistente).
- Servicios: `services.rs` (servidores MCP del catálogo de la app), `connections.rs` (inicio de sesión OAuth 2.1, sin
  clave que pegar), `connectors.rs` y `connectors_run.rs` (conectores que escribe un agente, ejecutados en su
  sandbox), `watchers.rs`.
- Herramientas servidas por MCP a cada agente (`agent_tools.rs`, `mcp_sessions.rs`): audio y pódcasts, `weather.rs`,
  `routes.rs`, mapas.

## Archivos, páginas y mapas

- `files.rs` guarda lo que la persona envía (menú compartir incluido) donde el sandbox del agente puede leerlo;
  `files_kept.rs` fija cuánto tiempo. `media.rs`, `sent_files.rs`, `give_file.rs`, `made_files.rs` y `cited_files.rs`
  aseguran que un archivo que el agente hace o menciona llegue de verdad a la persona.
- `pages.rs`: paneles, seguimientos y pequeñas herramientas que el agente escribe en un solo archivo HTML, bloqueados
  con una Content-Security-Policy (sin red, sin formularios) antes de mostrarse.
- `maps.rs`, `map_tiles.rs`, `map_route.rs`: mapas de OpenStreetMap a través del bridge, mapas interactivos, rutas
  reales por las calles; `favicons.rs` trae los iconos de los sitios para que no lo haga el móvil.

## Trabajo por su cuenta

- `jobs.rs`: tareas en segundo plano con un presupuesto elegido antes, los tokens mostrados después, pausa y
  reanudación; `handback.rs` devuelve el informe a la conversación de la que salió.
- `cronwatch.rs` entrega la respuesta de cada rutina en la app; `autonomous.rs` lista todo lo que los agentes hacen
  sin que se lo pidan; `storage_watch.rs` hace que los agentes pregunten qué guardar; `feature_tips.rs` muestra
  funciones poco a poco.

## Control de costes

- `tool_diet.rs` coloca las herramientas poco usadas detrás de la búsqueda de herramientas de Hermes según el tipo de
  agente (medido: unos 19k tokens de entrada por llamada, 11,8k de ellos esquemas de herramientas).
- `model_route.rs` fija un proveedor en EUrouter para que funcione su caché (entrada en caché a un 17,5 % del precio,
  aproximadamente); `model_choice.rs` deja elegir un modelo por agente a quien se aloja a sí mismo.
- El parche 0020 informa del consumo de cada llamada al modelo, que `jobs.rs` y `runs.rs` usan para parar una tarea
  antes de que supere su presupuesto. `credit.rs` pausa lo que funciona solo cuando se agota el crédito del mes, nunca
  las conversaciones.
- `sleep.rs` detiene Hermes en un espacio que nadie usa desde hace días y lo despierta con la siguiente petición.

## Instalación, copias y mudanzas

- El instalador ([`installer/`](../installer/)) monta Hermes, el bridge, las voces y los sandboxes con un comando y
  los actualiza. `place_backup.rs` expone las copias de seguridad y su clave; `place_import.rs` trae tu propio Hermes;
  `place_move.rs` muda un espacio a otro servidor; `ssh_keys.rs` permite a la persona entrar en su propia cuenta.
- `space.rs`: el almacenamiento del espacio cifrado en reposo (desactivado por defecto).

## La oferta todo incluido

En los servidores de Kerfy Studio cada persona tiene un **espacio**: su propia cuenta, su Hermes y su bridge.
`cloud_setup.rs` monta un espacio nuevo a partir de un servidor preparado; `place_cert.rs` le consigue un certificado
real de Let's Encrypt bajo `<label>.p.bipagents.com`; `proxy_header.rs` permite que cada espacio responda en el puerto
443 detrás de un router SNI (muchas redes bloquean otros puertos); `credit.rs` y `beta_measures.rs` hablan con el
guichet, el servicio de cuentas y facturación de Kerfy Studio (un repositorio privado, fuera de este).

## Los parches de Hermes que llevamos

Algunos cambios que necesitan las apps aún no están en Hermes. Están en
[`hermes/patches/`](../hermes/patches/README.md), sobre la versión estable fijada (`BASE`: v0.22.5), y el instalador
los aplica.

| Parche | Por qué |
|---|---|
| 0005 la caja fuerte rellena tras reiniciar el navegador | los inicios de sesión siguen funcionando después de cerrar el navegador del agente |
| 0006 respuesta síncrona de un par | un agente consultado por otro responde en la conversación de quien pregunta |
| 0007 `browser.vault_only` | los agentes con su propio navegador visible conservan la caja fuerte sin el navegador de Hermes |
| 0008 zona horaria, rutinas, hora local | las rutinas se ejecutan a la hora de casa, una vez por hora local |
| 0009 búsqueda de herramientas `!nombre`, categorías de habilidades compactas | la dieta de tokens: algunas herramientas directas, las habilidades raras solo por su nombre |
| 0010 respuestas vacías | las llamadas a herramientas que un modelo escribe como texto se vuelven llamadas reales; los reintentos cambian la petición |
| 0011 títulos con GLM | GLM responde vacío a las llamadas sin razonamiento, así que los títulos piden razonamiento « low » |
| 0013 resultados MCP: contraseñas de la caja fuerte borradas | una contraseña nunca vuelve al modelo por el resultado de una herramienta |
| 0014 resultados de rutinas en UTF-8 simple | los acentos siguen legibles en los resultados de las rutinas |
| 0015 ancla temporal | la hora actual viaja con cada mensaje |
| 0016 un solo `message_agent`, el del bridge | todo mensaje entre agentes pasa por los controles del bridge |
| 0017 elementos de dirección | las direcciones de la caja fuerte llevan nombre, teléfono y correo |
| 0018 turnos de invitados | el turno de un familiar solo tiene las herramientas del invitado y ningún USER.md |
| 0019 las rutinas informan en su conversación | una rutina creada en una conversación responde allí |
| 0020 consumo por llamada en la API Runs | el coste real de cada llamada al modelo, para los presupuestos |

**Cómo los llevamos upstream.** La serie es una rama git sobre Hermes, exportada con `scripts/hermes-patches.sh`
(`branch`, `export`, `check`): un commit por parche, nombres estables, los mismos bytes cada vez. Pasar a un Hermes
más reciente es un rebase de esa rama y una nueva `BASE`. Un parche desaparece en cuanto Hermes hace lo mismo: 0001
(clarify en la API Runs) y 0004 (imágenes guardadas para preguntas posteriores) se fueron cuando v0.22.5 trajo los
suyos. 0005 está propuesto upstream (NousResearch/hermes-agent#136660); los parches pequeños y generales (la
corrección de 0008, 0009, 0011, 0013, 0014, 0017, las etiquetas de 0019) son los siguientes candidatos. Detalle de
cada parche: [`hermes/patches/README.md`](../hermes/patches/README.md).

## Lo que dejamos a propósito a Hermes

El bucle del modelo y sus herramientas, los archivos de memoria y las habilidades, el planificador cron, el motor de
aprobaciones, el almacén de la caja fuerte y el rellenado de formularios, el Bot Mode, los perfiles, el backend docker
del terminal, el Bot Desktop, las sesiones y su base de datos, y las pasarelas. El bridge los configura (con la CLI y
los archivos de configuración del propio Hermes) y habla con Hermes por su API Runs; nunca hace un fork de Hermes. Lo
que es lo bastante general se ofrece upstream en vez de quedarse aquí.
