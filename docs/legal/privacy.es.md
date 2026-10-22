[English](privacy.en.md) · [Français](privacy.fr.md) · [Deutsch](privacy.de.md) · Español

# Política de privacidad de BipAgents

> Última actualización: [fecha].

## En resumen

- BipAgents conecta tu teléfono con **tus propios agentes de IA**, construidos sobre Hermes Agent. Funcionan **en tu
  ordenador o tu servidor** o, con la **oferta todo incluido**, en **tu plaza**, una cuenta en un servidor que Kerfy
  Studio alquila a un proveedor de alojamiento socio. Tus conversaciones, tus archivos, tu voz y la memoria de tus
  agentes se quedan en ese servidor y en el proveedor de modelos de IA que les responde. **Kerfy Studio, el editor de
  la app, no los lee.**
- Sin publicidad, sin estadísticas de uso, sin rastreo. Sin cuenta con tu propio servidor; una cuenta para la oferta
  todo incluido.
- Con tu propio servidor, Kerfy Studio solo puede recibir tres cosas:
  1. lo necesario para entregar tus **notificaciones** a través de su relé: un token de dispositivo sellado, nunca el
     texto de tus mensajes;
  2. lo que decidas enviarnos: un **correo** al soporte o la **denuncia** de una respuesta desde la app;
  3. **informes de fallos**, solo si has activado su envío en los ajustes de tu teléfono.
- Con la oferta todo incluido, Kerfy Studio trata además tu cuenta, el estado de tu suscripción, datos técnicos sobre
  tu plaza (nunca su contenido) y tus copias de seguridad, cifradas con una clave que no tiene; aloja tu plaza sin
  acceder a ella (sección 5).

## 1. Quién es responsable

**Kerfy Studio**, SAS (sociedad por acciones simplificada francesa) con un capital de [capital] €, con domicilio
social en [dirección del domicilio social], inscrita en el RCS de [ciudad del registro] con el número [SIREN]
(«Kerfy Studio», «nosotros»). Contacto: **support@kerfystudio.com**.

Kerfy Studio no ha designado un delegado de protección de datos: no es obligatorio para su actividad (artículo 37 del
RGPD). Para cualquier pregunta sobre tus datos: support@kerfystudio.com. Para la ley de Singapur (Personal Data
Protection Act), que exige una persona encargada de la protección de datos, es el presidente de Kerfy Studio,
[nombre y apellidos], a quien puedes contactar en la misma dirección.

Kerfy Studio es responsable de los tratamientos que la app realiza para funcionar, tal como Kerfy Studio los ha
diseñado (secciones 3, 4 y 7), incluso cuando se quedan en tu teléfono y Kerfy Studio no tiene acceso a ellos, y de
los tratamientos de la oferta todo incluido (sección 5). Cuando usas tu propio servidor, no es ni responsable ni
encargado del tratamiento de los datos que tratan ese servidor, tus agentes y tu proveedor de modelos (sección 2).

## 2. Cómo funciona BipAgents y quién ve qué

Con tu propio ordenador o tu propio servidor:

| Dónde | Qué hay | Quién decide |
|---|---|---|
| **Tu servidor** (tu ordenador o un servidor que alquilas) | Hermes Agent, tus agentes, tus conversaciones, tus archivos, su memoria, sus tareas programadas, la caja fuerte de contraseñas | Tú |
| **Tu proveedor de modelos** (una API en la nube, por ejemplo OpenRouter, o un modelo que funciona en tu casa) | Cada mensaje que tus agentes le envían para que responda | Tú, según las condiciones de ese proveedor |
| **Tu teléfono** | Lo que la app guarda para funcionar (sección 3) | Tú |
| **Kerfy Studio** | Solo lo que se describe en la sección 4 | Kerfy Studio |

- **BipAgents está construida sobre Hermes Agent, software libre de Nous Research**
  (https://github.com/NousResearch/hermes-agent), que instalas en tu propia máquina. La app se conecta a él
  directamente, a través de **Tailscale** (una red privada entre tus dispositivos) o de tu **red local**. Kerfy Studio
  no aloja tus agentes y no tiene ningún acceso a tu servidor. Para estos datos, Kerfy Studio no es ni responsable ni
  encargado del tratamiento.
- **Si alquilas un servidor**, su proveedor de alojamiento tiene acceso técnico a su disco. Elige uno en el que
  confíes (por ejemplo, en la Unión Europea).
- **Tu proveedor de modelos** recibe los mensajes a los que responde y los trata según sus propias condiciones y su
  propia política de privacidad. Algunos ofrecen no conservar nada («zero data retention»): eso lo eliges tú. Con un
  modelo que funciona en tu casa, nada sale de tu red.
- **Los servicios que conectas a tus agentes** (correo, agenda, sitios web, domótica…) reciben lo que tus agentes les
  envían, según sus propias reglas.
- **Varias personas en un mismo servidor** (función familia): cada persona tiene su propia instalación, separada de
  las demás. Quien administra el servidor es responsable frente a las demás.

**Con la oferta todo incluido**, tu plaza ocupa el lugar de tu servidor, y Kerfy Studio elige el proveedor de
alojamiento y el enrutador de IA: la sección 5 dice quién ve qué.

## 3. Lo que la app guarda en tu teléfono

Estos datos se quedan en tu teléfono y **no** se envían a Kerfy Studio. Lo que aquí se dice del iPhone vale también
para el iPad.

- **Tus agentes**: su nombre, sus direcciones en tu servidor, la huella del certificado de tu red local, sus ajustes
  (mascota, voz, idioma).
- **Las claves de acceso a tus agentes**: en el Llavero del iPhone (limitadas a este dispositivo) o cifradas con una
  clave del Keystore de Android.
- **Una copia de tus conversaciones recientes**, para leerlas sin conexión, tus borradores, los mensajes que se están
  enviando y las respuestas que llegaron durante tu ausencia (el Buzón).
- **Tus notas de voz** (grabaciones y transcripciones), los archivos que envías y los que tus agentes te envían.
- **El widget**, si lo añades, muestra el último mensaje de un agente, también en la pantalla bloqueada del iPhone si
  lo colocas allí.
- **En iPhone**, las notificaciones de tus agentes se muestran como mensajes: iOS guarda en el dispositivo el nombre
  del agente y el texto mostrado, como en una app de mensajería.
- **El navegador del agente** (vista Escritorio): en iPhone, no se guarda nada; en Android, sus cookies se quedan en
  la app.

**Por qué.** Estos tratamientos solo sirven para ofrecerte las funciones que usas (artículo 6.1.b del RGPD). Estos
datos se quedan en tu teléfono hasta que elimines el agente o la app.

**Copias de seguridad.** En iPhone, estos datos forman parte de tu copia de seguridad de iCloud o de tu ordenador si
la has activado, salvo las claves, que se quedan en el dispositivo. En Android, la app excluye sus datos de las
copias de seguridad; una transferencia directa a un teléfono nuevo puede copiar una parte, pero no las claves, que
allí no se pueden leer.

**Borrar.** Elimina un agente en la app, o desinstala la app para borrarlo todo del teléfono. Esto no borra lo que
está en tu servidor.

**Lo que compartes tú.** El informe de diagnóstico (Ajustes › Diagnóstico y latencias › Exportar el informe) contiene
el nombre y la dirección de tus agentes; solo va a la app o a la persona que eliges en la hoja para compartir.

**Tu salud, si la activas (« Santé », Salud).** Para cada agente, puedes activar Salud en sus herramientas. Con tu
permiso en Apple Salud (iPhone, incluidos los datos de tu Apple Watch) o Health Connect (Android), la app lee, sin
escribir nunca nada allí, un resumen de cada día: tu sueño (tiempo dormido, tiempo en la cama, hora de acostarte y de
levantarte), tus pasos, tu energía activa, tus minutos de ejercicio, tu frecuencia cardíaca en reposo, tu
variabilidad cardíaca, tu peso y tus sesiones de deporte (tipo, duración, distancia, energía). Envía los últimos 30
días la primera vez y luego los días que cambiaron, **solo a tu servidor** (o a tu plaza con la oferta todo incluido),
donde se guardan en un archivo por día, 400 días como máximo. Solo los agentes para los que activaste Salud los leen,
para ayudarte con tu sueño, tu actividad y tu entrenamiento; no hacen diagnósticos. El modelo de IA del agente recibe
lo que el agente lee para responderte: con la oferta todo incluido, Salud solo es posible con un modelo que funciona en
la Unión Europea. El teléfono solo guarda las fechas y una huella de lo que ya envió, nunca los valores, y nada en
iCloud. Kerfy Studio no los recibe; nada se vende ni se usa para publicidad. **Base jurídica**: tu consentimiento
explícito (artículos 6.1.a y 9.2.a del RGPD), que das al activar Salud y permitirlo en Apple Salud o Health Connect.
**Dejarlo**: desactiva Salud; cuando ya ningún agente la tiene, los días guardados en tu servidor se borran. También se
borran con el agente, con tu plaza, o al retirar el permiso en Apple Salud o Health Connect (ya no se envía nada).

**Tu agenda, si la compartes (« Calendrier », Calendario).** Para cada agente, puedes activar Calendario en sus
herramientas. Con tu permiso (Calendarios en el iPhone, Calendario en Android), la app lee las agendas de tu teléfono,
incluidas las cuentas añadidas en él (Google, Outlook…), y envía una copia de sus citas, desde una semana antes hasta
dos meses después de hoy (título, horas, lugar, notas), **solo a tu servidor** (o a tu plaza con la oferta todo
incluido), cuando se abre la app. La copia se queda allí y se sustituye en cada envío. Solo la leen los agentes que
elegiste; cuando piden añadir, cambiar o borrar una cita, lo hace tu teléfono. El modelo de IA del agente recibe las
citas que el agente lee para responderte. Kerfy Studio no las recibe. **Base jurídica**: tu consentimiento (artículo
6.1.a del RGPD), que das al activar Calendario y permitirlo en tu teléfono. **Dejarlo**: desactiva Calendario para el
agente, o retira el permiso en los ajustes del teléfono (ya no se envía nada). Para un buzón que conectaste, también puedes activar
« Usar también su agenda »: tu servidor lee entonces los calendarios de esa cuenta (CalDAV, con las credenciales del
buzón, que quedan cifradas en tu servidor) cuando un agente de ese buzón los consulta, y añade o borra las citas que
pide; no se guarda nada salvo la lista de esos calendarios. Desactívalo para dejarlo.

**Los recordatorios por lugar y tu ubicación.** Cuando le pides a un agente «recuérdame la leche al llegar al
súper», pide un recordatorio con el nombre del lugar (o sus coordenadas). En el iPhone, es la app Recordatorios de
Apple la que vigila el lugar: BipAgents no tiene acceso a tu ubicación; el nombre del lugar se busca en Mapas de Apple
para encontrar sus coordenadas. En Android, que no tiene app de recordatorios, BipAgents guarda él mismo estos
recordatorios: te pide el permiso de ubicación (también en segundo plano) solo cuando un agente ha creado uno, busca el
lugar con el servicio de geocodificación del teléfono y confía el círculo alrededor del lugar a los servicios de
ubicación de Google Play en el teléfono (geofencing). Tu ubicación se queda en el teléfono: no se envía ni a tu
servidor, ni al agente, ni a Kerfy Studio, y solo sirve para avisarte al llegar o al salir. **Base jurídica**: tu
consentimiento (artículo 6.1.a del RGPD), que das al permitir la ubicación. **Para dejarlo**: elimina el recordatorio
(Ajustes › Recordatorios por lugar) o retira el permiso de ubicación en los ajustes del teléfono.

**Lo que compartes con la app.** Cuando compartes un mensaje o una captura de pantalla con BipAgents (desde WhatsApp,
Signal, Mensajes…), va al agente que eliges, en tu servidor, como un adjunto que envías tú; puede contener palabras de
otras personas, así que comparte solo lo que necesites. La app nunca envía el borrador de respuesta que escribe el
agente: «Enviar con…» abre la app que eliges, y lo envías tú.

## 4. Lo que Kerfy Studio puede recibir

### 4.1 Las notificaciones, a través del relé de Kerfy Studio

**Por qué un relé.** Para enviarte una notificación, tu servidor tiene que pasar por Apple (iPhone) o Google
(Android), con claves que pertenecen al editor de la app y que no se pueden confiar a cada servidor. El relé de Kerfy
Studio las tiene y transmite las notificaciones de tu servidor.

**Cómo.** Cuando autorizas las notificaciones, Apple o Google dan a la app un **token de dispositivo**. La app lo
envía al relé, que se lo devuelve **sellado** (cifrado con una clave que solo tiene el relé). Tu servidor guarda este
token sellado y lo envía al relé con cada notificación; el relé lo abre, lo transmite a Apple o Google y luego lo
olvida todo.

**Lo que trata el relé:**

- el token de dispositivo, la plataforma (Apple o Google) y el entorno (prueba o producción);
- la **dirección IP** de tu teléfono (al registrarse) o de tu servidor (con cada notificación), solo para limitar
  abusos: se transforma en una huella, se guarda en memoria una hora como máximo y nunca se escribe;
- el contenido de la notificación, reducido al **nombre del agente** (el título), a un **texto genérico** como «Nuevo
  mensaje» (cualquier otro texto se sustituye) y a **identificadores técnicos** (agente, conversación, tarea, tipo de
  notificación). **Nunca el texto de tus mensajes**: la app lo busca ella misma en tu servidor.

**Lo que se conserva.** El relé no tiene base de datos. El token sellado se guarda en tu teléfono y en tu servidor; con
la oferta todo incluido, el guichet de Kerfy Studio también lo guarda, siempre sellado (sección 5.6). Los registros
técnicos del relé contienen la plataforma, una huella de 4 bytes del token y la respuesta de Apple o de Google; se
conservan **6 meses como máximo**. El proveedor de alojamiento del relé puede registrar las direcciones IP de las
solicitudes: [lo que registra el proveedor de alojamiento del relé y durante cuánto tiempo].

**Finalidades y bases jurídicas.** Entregar las notificaciones que has activado: ejecución de las condiciones de uso
(artículo 6.1.b del RGPD). Limitar abusos y proteger el servicio: interés legítimo de Kerfy Studio (artículo 6.1.f).

**Encargados del tratamiento.** [proveedor de alojamiento del relé] (Unión Europea) y Google (Firebase Cloud
Messaging), vinculados a Kerfy Studio por un contrato conforme al artículo 28 del RGPD. Apple entrega las
notificaciones a los iPhone a través de su servicio APNs.

**Sin notificaciones.** Puedes rechazar o desactivar las notificaciones en los ajustes de tu teléfono: la app funciona
sin ellas. Si desinstalas la app, Apple o Google marcan el token como inválido y tu servidor lo olvida. Si compilas
la app tú mismo con tu propia cuenta de Apple, no se usa el relé de Kerfy Studio.

**En Android**, la app solo pide un token a Google en el momento en que activas las notificaciones.

### 4.2 El soporte y las denuncias

- **Ajustes › Contactar con soporte** prepara un correo a support@kerfystudio.com con un diagnóstico que puedes leer
  antes de enviarlo: versión de la app, modelo y versión del sistema del teléfono, idioma, estilo de visualización,
  modo demo o no, estado de las notificaciones y, para cada agente, su categoría, si responde, si tiene un bridge y
  una puerta en la red local, y su voz. **Ninguna conversación, ningún nombre de agente, ninguna dirección, ninguna
  clave.** No se envía nada hasta que pulsas «Enviar» en tu app de correo. También recibimos tu dirección de correo
  y todo lo que decidas escribir: evita poner información sensible.
- **Denunciar esta respuesta** (mantener pulsada una respuesta) envía la denuncia **desde la app**, a través del relé
  de Kerfy Studio, solo a Kerfy Studio: ni a tu servidor ni al proveedor del modelo. Contiene el motivo, tus
  precisiones opcionales, **el texto de la respuesta denunciada**, la categoría del agente, la versión de la app y el
  modelo del teléfono. **Ni tu dirección de correo, ni tu nombre, ni el del agente.** No se envía nada hasta que
  pulsas «Enviar». El relé ve de paso la dirección IP de tu teléfono y solo la usa para limitar abusos (sección 4.1).
  Como la denuncia no contiene ningún medio para contactarte, no podemos responderla: escribe al soporte si esperas
  una respuesta.
- **Si el relé no responde**, la app te propone enviar la misma denuncia por correo: entonces también recibimos tu
  dirección de correo.

El texto de la respuesta denunciada puede hablar de tu salud o de otras personas: reléelo antes de enviarlo y escribe
en las precisiones solo lo que sea útil. Borramos lo que no sirve para tramitar la denuncia.

**Finalidades.** Responderte, tramitar las denuncias, corregir errores y mejorar la app.
**Bases jurídicas.** Responder a tus mensajes y tramitar tus denuncias: interés legítimo de Kerfy Studio en
responderte y hacer la app más segura (artículo 6.1.f del RGPD). Responder a una solicitud de ejercicio de tus
derechos: obligación legal (artículo 6.1.c; artículos 12 a 22 del RGPD).
**Plazo.** Soporte: 2 años después del último intercambio. Denuncias: 1 año después de su tramitación.
**Destinatarios.** Kerfy Studio y sus encargados del tratamiento: su proveedor de correo ([proveedor de correo],
[país]) y el proveedor de alojamiento del relé, por el que pasan las denuncias.

### 4.3 Los informes de fallos

Si has activado, en los ajustes de tu teléfono, el envío de datos de análisis a los desarrolladores de apps (iPhone:
Ajustes › Privacidad y seguridad › Análisis y mejoras › Compartir con desarrolladores de apps; Android: envío de datos
de uso y diagnóstico), Apple o Google pueden transmitir a Kerfy Studio informes de fallos y mediciones técnicas
(modelo, versión del sistema, lugar del código donde la app falló). La app no contiene ninguna herramienta de
estadísticas ni de informes de fallos de terceros: estos informes solo vienen de Apple o de Google.

**Finalidad.** Corregir errores.
**Base jurídica.** Interés legítimo de Kerfy Studio en corregir errores (artículo 6.1.f del RGPD). Estos informes solo
nos llegan si has activado este envío en los ajustes de tu teléfono, donde puedes desactivarlo en cualquier momento.
Apple y Google los recogen bajo su propia responsabilidad.
**Plazo.** Kerfy Studio borra los informes que descarga como máximo 1 año después de recibirlos.

### 4.4 Las estadísticas de las tiendas

Apple y Google proporcionan a Kerfy Studio estadísticas sobre la app (descargas, países, versiones). Apple y Google
las elaboran bajo su propia responsabilidad y nos las dan agregadas: no nos permiten identificarte.

## 5. La oferta todo incluido

Esta sección se refiere a las personas que usan la oferta todo incluido ([condiciones de la oferta](offer.es.md)): una
plaza en un servidor que Kerfy Studio alquila a un proveedor de alojamiento socio, un crédito de IA, copias de
seguridad y una cuenta. El resto de esta política también se aplica (la app en tu teléfono, las notificaciones, el
soporte). El servicio de Kerfy Studio que gestiona las cuentas, las suscripciones y las plazas se llama guichet.

### 5.1 Quién es responsable de qué

- **Kerfy Studio es el responsable** de los tratamientos que requiere la oferta: tu cuenta, el estado de tu
  suscripción, el alojamiento de tu plaza, el envío de tus mensajes al modelo de IA, las copias de seguridad, la
  supervisión técnica y la tramitación de los abusos.
- **El contenido de tu plaza** (tus agentes, tus conversaciones, tus archivos, su memoria, sus rutinas, la caja fuerte
  de contraseñas): tú decides lo que pones en ella. Kerfy Studio lo aloja y hace que lo trate el modelo de IA solo para
  prestarte el servicio (artículo 6.1.b del RGPD), **sin acceder a él**: su software no lo lee
  (las únicas órdenes que ejecuta un servidor son reinstalar una plaza, actualizarla a una versión publicada del código abierto de BipAgents o volver a abrirla, y ninguna lee su contenido). El acceso a las cuentas de Kerfy Studio en los proveedores de alojamiento está reservado a la sociedad. Solo
  se accedería al contenido de una plaza para cumplir una obligación legal (una orden judicial, por ejemplo), y se te
  informaría cuando la ley lo permita. Como todo proveedor de alojamiento, OVHcloud y Hetzner tienen acceso físico a
  sus servidores. Si pones en ella datos sensibles (sobre tu salud, por ejemplo), solo se tratan para ejecutar lo que
  pides a tus agentes.
- **Apple y Google** son responsables de sus propios tratamientos cuando inicias sesión con ellos y cuando pagas en su
  tienda.
- **Tus agentes son IA y lo dicen**: la insignia «IA», un aviso al principio de cada sesión Live, una mención en las
  notificaciones y en los mensajes que envían a otras personas (Reglamento (UE) 2024/1689, artículo 50).

### 5.2 Tu cuenta

**Datos.** El identificador que te da Apple o Google y tu dirección de correo (con Apple, puede ser una dirección de
reenvío); o, con una dirección de correo, los códigos de un solo uso que se te envían. La lista de tus dispositivos
aprobados, la clave pública de cada uno (lo que otro de tus dispositivos le envía va sellado para él) y sus tokens de
notificación, sellados por el relé
(sección 4.1): Kerfy Studio nunca los ve en claro. Las invitaciones que envías (la dirección de correo de la persona
invitada y su estado). Una solicitud de recuperación en curso. La copia de tu clave de copia de seguridad, cifrada con
tu código de recuperación, que Kerfy Studio no puede leer. El crédito de IA comprado y conservado para una próxima
suscripción (un importe). La fecha de eliminación de la cuenta, si la has solicitado. Ni tu nombre, ni tu foto, nunca
las claves de tu plaza.

**Finalidades.** Iniciar tu sesión, vincular tu suscripción y tus plazas, aprobar tus dispositivos, recuperar tu
cuenta, enviarte los códigos de inicio de sesión, los avisos de seguridad (dispositivo nuevo, recuperación en curso),
la confirmación de tu compra, el acuse de recibo de un desistimiento y los avisos de eliminación, gestionar las
invitaciones. Ningún correo publicitario: un mensaje comercial solo se enviaría si das tu acuerdo marcando una casilla.
**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).
**Plazo.** Mientras exista tu cuenta. Cuando la eliminas, se borra **30 días** después de tu solicitud (tus plazas se
detienen desde la solicitud, y puedes cambiar de opinión hasta entonces), con sus plazas y sus copias de seguridad,
salvo lo que la ley obliga a conservar: como proveedor de alojamiento, Kerfy Studio conserva aparte, y solo para
responder a las autoridades judiciales, tu dirección de correo 5 años y tu identificador de Apple o de Google 1 año
después del fin del contrato (ley francesa para la confianza en la economía digital, artículo 6, II; decreto
n.º 2021-1362, artículos 2, 3 y 8). Códigos de un solo uso: el tiempo de su validez. Invitación no aceptada: 30 días.
**Eliminar tu cuenta.** En la app, Ajustes › Cuenta › Eliminar mi cuenta, o, si ya no tienes la app, en la página
https://guichet.kerfystudio.com/account/delete con un código enviado a tu dirección de correo. Las personas con las que
compartes tu plan reciben un aviso por correo: sus plazas se detienen el día en que se borra tu cuenta.

### 5.3 Tu suscripción, tus compras y la semana gratis

**Datos recibidos de las tiendas.** A través de las notificaciones de Apple (App Store Server Notifications) y de
Google (Google Play, notificaciones en tiempo real): el identificador de tu suscripción en la tienda, el plan, la
región, su estado (semana gratis, activa, suspendida, terminada, reembolsada), el final del periodo pagado, la moneda y el precio mensual del plan, si la
renovación está desactivada y, para el crédito de IA que compras, el identificador de cada compra y su importe.
**Nunca tus medios de pago**, que solo conoce la tienda. El país de tu cuenta de la tienda, que la app envía para
saber si la oferta se vende allí (no se conserva).

**Tu contrato.** La fecha de inicio, la del correo de confirmación y, si desistes del contrato, la fecha y la hora de
tu desistimiento.

**Crédito de IA usado.** El importe que la clave de IA de cada plaza ha gastado en el mes, consultado en el enrutador
de IA una vez por minuto como máximo, y el crédito traspasado: un importe, nunca tus mensajes.

**Solo una semana gratis por dispositivo.**

- **En iPhone**, la app usa DeviceCheck de Apple: Apple guarda dos bits por dispositivo para Kerfy Studio («ya tuvo
  una semana gratis»). Kerfy Studio no recibe ningún identificador del dispositivo: la app le da un token de un solo
  uso que transmite a Apple.
- **En Android**, la app envía un token de Google Play Integrity (Google confirma que la app es auténtica y funciona en
  un dispositivo real) y una huella del identificador Android del dispositivo, calculada en el teléfono (SHA-256) y
  después vuelta a calcular por el guichet con una clave secreta antes de guardarla. Kerfy Studio solo conserva esta
  segunda huella, y solo para este límite.
- **La dirección IP** de tu teléfono se conserva mientras dura la reserva de una plaza de prueba (15 minutos como
  máximo), para limitar el número de reservas por dirección.

**Finalidades y bases jurídicas.** Prestar y gestionar tu suscripción, tus plazas y tu crédito: ejecución del contrato
(artículo 6.1.b del RGPD). Evitar que una misma persona encadene semanas gratis: interés legítimo de Kerfy Studio en
prevenir el fraude y en poder seguir ofreciendo una semana gratis (artículo 6.1.f y considerando 47). Leer el
identificador del dispositivo no requiere tu consentimiento, porque solo sirve para limitar el acceso gratuito al
servicio a un periodo limitado (artículo 82 de la ley francesa de protección de datos; CNIL, deliberación
n.º 2020-091, artículo 5). Conservar la prueba del contrato: interés legítimo de Kerfy Studio en poder defender sus
derechos.
**Plazo.** Datos de suscripción y de compra: durante la suscripción y después 5 años archivados, como prueba del
contrato (artículo 2224 del Código Civil francés). Huella del dispositivo Android: 24 meses. Dirección IP: 15 minutos
como máximo. Los bits de DeviceCheck los guarda Apple.

### 5.4 Tu plaza y el modelo de IA

**Dónde.** En un servidor alquilado a OVHcloud (centros de datos en la Unión Europea) o a Hetzner (Alemania o
Finlandia), para todas las personas de la oferta, también las que viven fuera de la Unión.

**Lo que hay.** Lo que describe la sección 5.1, y las claves SSH públicas que añades desde la app. El guichet de Kerfy
Studio (el servicio que gestiona las plazas) conoce la dirección de tu plaza y la huella de su certificado, para que tu
teléfono se conecte a ella y para trasladarla en caso de avería.

**El modelo de IA.** Los mensajes que tus agentes envían al modelo (con los archivos y las fotos que le muestran)
pasan por un enrutador de IA, **EUrouter** (EUrouter B.V., Ámsterdam, Países Bajos), para todas las personas de la
oferta, y después por el proveedor que hace funcionar el modelo elegido: [por completar: proveedores de los modelos
ofrecidos y países donde funcionan]. EUrouter indica que por defecto no guarda ni las solicitudes ni las respuestas, y
que guarda sus metadatos (hora, modelo, número de tokens, coste, dirección IP) hasta 12 meses. Actúan por cuenta de
Kerfy Studio. **Antes del primer mensaje, la app nombra el enrutador y el fabricante del modelo y te pide tu
acuerdo**; la lista de modelos indica el fabricante de cada uno.

**El reposo.** Tu plaza anota ella misma la última solicitud de la app; el guichet solo sabe que duerme.

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).
**Plazo.** Durante la suscripción. Cuando termina, tu plaza se conserva 30 días (3 días después de una semana gratis
que no continuó) y después su cuenta se borra y se reinstala para otra persona. Las plazas de más de un plan reducido
se borran 30 días después de detenerse.

### 5.5 Las copias de seguridad

Tu plaza hace una copia de seguridad cada hora. La copia se **cifra en la plaza**, antes de salir, con una clave que
solo tienen tus dispositivos, más una copia de esa clave cifrada con tu código de recuperación (sección 5.2). Kerfy
Studio guarda estas copias en [por completar: proveedor de alojamiento de las copias de seguridad] sin poder leerlas:
una por hora durante 48 horas, una por día durante 30 días, una por semana durante 12 semanas. Se borran con tu plaza.

**La imagen diaria de OVHcloud.** En sus servidores, OVHcloud hace además una imagen diaria del servidor entero, que
guarda en sus instalaciones: un punto de restauración por día, no cifrado por Kerfy Studio. Cuando una plaza se
reinstala para otra persona, la imagen que la contenía se sustituye en un día aproximadamente.

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).

### 5.6 La supervisión técnica y las averías

Cada dos minutos, cada servidor envía al guichet: su memoria, su disco y su carga y, para cada plaza, si su bridge
responde, si su Hermes Agent funciona o duerme, su versión de BipAgents y la hora de su última copia de seguridad
correcta. **Nunca contenido.** Estos datos sirven para detectar una avería, trasladar las plazas, hacer las
actualizaciones por oleadas y avisar a Kerfy Studio.

Cuando el servidor de tu plaza se avería, se traslada o vuelve, tus teléfonos reciben un aviso a través del relé de
notificaciones, con un texto genérico: para ello, el guichet guarda los tokens sellados de tus teléfonos vinculados a
tu plaza.

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).
**Plazo.** Solo se guardan los últimos valores, sustituidos en cada envío. Registro de eventos (averías, traslados,
actualizaciones): 12 meses. Tokens sellados: mientras el dispositivo esté vinculado a tu plaza.

### 5.7 Las denuncias de abusos y las solicitudes de las autoridades

**Datos.** La denuncia (quién la envía y cómo contactarle, los hechos, la dirección IP y la hora), la plaza afectada,
nuestra decisión y sus motivos, nuestros intercambios contigo.
**Finalidades.** Tramitar las denuncias ([política de uso aceptable](acceptable-use.es.md)), proteger los servidores y
a las demás personas, responder a las autoridades.
**Bases jurídicas.** Obligación legal (artículo 6.1.c del RGPD; Reglamento (UE) 2022/2065, artículos 16 a 18) e interés
legítimo de Kerfy Studio en proteger su servicio y a sus clientes (artículo 6.1.f).
**Plazo.** 1 año después de su tramitación, más tiempo si un litigio o un procedimiento lo exige.

### 5.8 Los prestadores de la oferta

| Prestador | Papel | Datos | Dónde |
|---|---|---|---|
| OVHcloud (OVH SAS, Roubaix, Francia) | Aloja los servidores de las plazas; imagen diaria de cada servidor | Contenido de las plazas | Unión Europea |
| Hetzner Online GmbH (Gunzenhausen, Alemania) | Aloja parte de los servidores de las plazas | Contenido de las plazas | Alemania, Finlandia |
| EUrouter B.V. (Ámsterdam, Países Bajos) | Enrutador de IA | Mensajes enviados al modelo, metadatos | Unión Europea |
| Proveedores de los modelos: [por completar] | Hacen funcionar el modelo elegido | Mensajes enviados al modelo | [por completar] |
| [por completar: proveedor de alojamiento de las copias de seguridad] | Guarda las copias de seguridad | Copias de seguridad cifradas, ilegibles para él y para Kerfy Studio | [por completar] |
| [proveedor de alojamiento del relé] | Aloja el guichet y el relé | Cuenta, suscripción, datos técnicos | Unión Europea |
| [por completar: proveedor de envío de correos] | Envía los códigos, los avisos de seguridad, las invitaciones, la confirmación de compra, el acuse de desistimiento y los avisos de eliminación | Dirección de correo, contenido de esos correos | [por completar] |

Están vinculados a Kerfy Studio por un contrato conforme al artículo 28 del RGPD. Apple (DeviceCheck, App Store) y
Google (Play Integrity, Google Play) también intervienen, como dice la sección 5.3; «Iniciar sesión con Apple» y el
inicio de sesión con Google son de su propia responsabilidad.

## 6. Lo que Kerfy Studio no hace

- Sin publicidad, sin herramienta de estadísticas, sin rastreo entre apps o sitios. Sin cuenta, salvo para la oferta
  todo incluido.
- Sin venta ni alquiler de datos.
- Sin entrenamiento de modelos de IA con tus datos por parte de Kerfy Studio.
- Sin acceso a tu servidor, al contenido de tu plaza, a tus conversaciones ni a tus archivos.

## 7. Los demás servicios que intervienen en la app

Además de tu servidor, la app recurre a estos servicios, que aplican sus propias reglas. Kerfy Studio no recibe nada
de estos intercambios. Kerfy Studio ha decidido usarlos en la app y responde de lo que sale de tu teléfono; después,
cada servicio es responsable de lo que hace con ello.

- **Iconos de los sitios.** Para mostrar el pequeño icono de los sitios citados en las respuestas, la app pide estos
  iconos a tu servidor, que los obtiene de DuckDuckGo (icons.duckduckgo.com): DuckDuckGo recibe el **nombre del
  sitio** (por ejemplo «ejemplo.es») y ve la dirección de tu servidor, no la de tu teléfono. No recibe el mensaje.
- **Reconocimiento de voz.** Las notas de voz y Live usan el reconocimiento de voz de tu teléfono. La app pide el
  reconocimiento en el dispositivo cuando tu teléfono lo ofrece para tu idioma. Si no, Apple o Google (en Android, el
  servicio de reconocimiento del sistema, a menudo el de Google) tratan el audio en sus servidores, bajo su propia
  responsabilidad. Lo que va a tu servidor es el texto transcrito. Base jurídica: ofrecerte la función que usas
  (artículo 6.1.b del RGPD).
- **Síntesis de voz del teléfono.** Cuando la voz del teléfono lee un texto (en la demo, o si tu servidor no
  responde), usa el motor de síntesis de tu teléfono, según sus ajustes.
- **En Android, los códigos QR** se leen en el teléfono, con una biblioteca integrada en la app, sin ningún servicio de
  Google.
- **En Android, el escáner de documentos** es una función de los servicios de Google Play (Google ML Kit). Google
  indica que ML Kit le envía información sobre el dispositivo y la app, identificadores de instalación y mediciones de
  uso y rendimiento, para el diagnóstico y sus propias estadísticas. La app te lo dice y te pide tu acuerdo antes del
  primer uso del escáner; sin ese acuerdo, el escáner no se abre, y siempre puedes adjuntar una foto o un archivo.
  Puedes cambiar de opinión en cualquier momento en Ajustes.
- **Apple y Google** para las notificaciones (sección 4.1).
- **Tailscale**, si lo usas: una app distinta, con su propia cuenta y su propia política de privacidad.
- **Los enlaces que abres** (sitios citados por tus agentes, página de Hermes Agent) se abren en tu navegador.

## 8. La demo

La demo integrada funciona por completo en tu teléfono: agentes y datos inventados, sin cuenta, sin servidor, y no se
envía nada a Kerfy Studio. Puedes abrirla desde la primera apertura, con «Solo la demo». Su pódcast es un archivo de
audio grabado de antemano e incluido en la app (voces: Fish Audio Drama 3, «Built with Fish Audio»). Si hablas con la
demo, se aplica el reconocimiento de voz del teléfono (sección 7).

## 9. Las voces

Las voces de los Bips se sintetizan **en tu teléfono** cuando este lo permite: la app descarga las voces de tu idioma
desde el App Store o Google Play, y tu servidor (o tu plaza) le envía el texto que hay que leer, ya preparado. Si no,
se sintetizan en tu servidor o en tu plaza. Si conectas tu propio modelo de voz, es un servidor que tú mismo haces
funcionar. Kerfy Studio no recibe nada de lo que leen las voces.

## 10. Enlaces de socios (alojamiento, más adelante)

La app o el sitio kerfystudio.com podrán proponer más adelante proveedores de alojamiento mediante enlaces de socios
(afiliación). En ese caso:

- estos enlaces se señalarán con la mención «Enlace de socio»;
- ni la app ni el sitio instalarán ningún rastreador para la afiliación: el código de socio solo figurará en la
  dirección del enlace;
- si sigues uno de estos enlaces, el proveedor podrá saber que tu visita viene de BipAgents, y Kerfy Studio podrá
  recibir una comisión si contratas;
- la app no enviará ningún dato personal al proveedor; lo que le des después se rige por su propia política de
  privacidad.

Antes del primer enlace de socio, esta sección precisará lo que Kerfy Studio recibe de los socios, y ese cambio se
anunciará como se indica en la sección 17.

## 11. Transferencias fuera de la Unión Europea

El relé de notificaciones está alojado en la Unión Europea ([proveedor de alojamiento del relé]). Para entregar las
notificaciones, Apple y Google pueden tratar el token y la notificación fuera de la Unión Europea, en particular en
Estados Unidos:

- Google LLC (Firebase Cloud Messaging) está certificada en el «EU-U.S. Data Privacy Framework», reconocido como
  garante de una protección adecuada por la Decisión de Ejecución (UE) 2023/1795 de la Comisión, de 10 de julio de
  2023;
- Apple indica que enmarca las transferencias desde el Espacio Económico Europeo con las cláusulas contractuales tipo
  adoptadas por la Comisión Europea.

**La oferta todo incluido.** Los servidores de las plazas están en la Unión Europea, para todas las personas de la
oferta. EUrouter trata los mensajes en la Unión Europea [por completar: países donde funcionan los modelos ofrecidos, y
la garantía si alguno está fuera de la Unión]. Apple y Google (inicio de sesión, tiendas, DeviceCheck, Play Integrity)
pueden tratar datos en Estados Unidos, con las garantías indicadas arriba.

**Si vives fuera de la Unión Europea** (Estados Unidos, Reino Unido, Israel, Singapur), tus datos vienen a la Unión
Europea, donde Kerfy Studio los trata conforme al RGPD, que se aplica a todos sus tratamientos. El Reino Unido e Israel
reconocen que la Unión Europea ofrece una protección adecuada; para Singapur, ese nivel de protección y nuestros
contratos con nuestros prestadores dan la protección comparable que exige la ley de Singapur.

Proveedor de correo: [país; garantía]. Proveedor de envío de correos y proveedor de alojamiento de las copias de
seguridad: [por completar: país; garantía]. Puedes pedirnos una copia de las garantías aplicables.

## 12. Plazos de conservación

| Datos | Plazo |
|---|---|
| Datos de la app en tu teléfono | Hasta que elimines el agente o la app |
| Datos de salud (« Santé ») en tu servidor o tu plaza | 400 días como máximo; se borran cuando ningún agente tiene Salud, con el agente o con la plaza |
| Token de dispositivo en el relé | Ninguna conservación: solo existe sellado, en tu teléfono y tu servidor (y en el guichet con la oferta todo incluido, mientras el dispositivo esté vinculado a tu plaza) |
| Dirección IP en el relé | En memoria, una hora como máximo, en forma de huella |
| Registros técnicos del relé | 6 meses como máximo |
| Correos de soporte | 2 años después del último intercambio |
| Denuncias | 1 año después de su tramitación |
| Informes de fallos | 1 año después de su recepción (en Kerfy Studio) |
| Cuenta de la oferta (identificadores, correo, dispositivos, invitaciones, crédito conservado) | Mientras exista la cuenta; se borra 30 días después de una solicitud de eliminación |
| Invitación no aceptada | 30 días |
| Dirección de correo e identificador de cuenta después del fin del contrato (obligación de los proveedores de alojamiento) | 5 años para el correo, 1 año para el identificador, conservados aparte |
| Suscripción y compras | Durante la suscripción y después 5 años archivados |
| Huella del dispositivo Android, calculada con una clave secreta (semana gratis) | 24 meses |
| Dirección IP al reservar una semana gratis | 15 minutos como máximo |
| Contenido de tu plaza | Durante la suscripción y después 30 días (3 días después de una semana gratis que no continuó) |
| Copias de seguridad | 48 horas (por hora), 30 días (diarias), 12 semanas (semanales); se borran con la plaza |
| Imagen diaria de OVHcloud | Un punto por día, sustituido en un día aproximadamente después de reinstalar la plaza |
| Datos técnicos de los servidores | Solo los últimos valores |
| Registro de eventos del guichet | 12 meses |
| Metadatos en EUrouter | Hasta 12 meses, según EUrouter |
| Denuncias de abusos | 1 año después de su tramitación |

## 13. Seguridad

El relé solo acepta conexiones cifradas (HTTPS), sella los tokens (XChaCha20-Poly1305), no tiene base de datos y
nunca escribe un token en claro. En la app, las claves de tus agentes están protegidas por el Llavero o el Keystore,
y la conexión con tu servidor pasa por Tailscale o por una puerta HTTPS en tu red local, que solo acepta el
certificado de tu servidor. El código de la app, del servidor y del relé es público: [URL del repositorio]. Para
informar de un fallo de seguridad: **support@kerfystudio.com** (consulta el archivo SECURITY.md del repositorio).

Con la oferta todo incluido, cada plaza es una cuenta separada del servidor, sin derechos de administrador; Kerfy
Studio no tiene ninguna clave SSH en los servidores, y un servidor solo ejecuta órdenes firmadas por el guichet. La app
se conecta a tu plaza en HTTPS y comprueba su certificado. Las copias de seguridad se cifran en la plaza antes de
salir.

**En caso de violación de datos**, Kerfy Studio avisa a la CNIL en un plazo de 72 horas (artículo 33 del RGPD) y a las
personas afectadas cuando el riesgo para ellas es alto (artículo 34); para las personas en Singapur, a la Personal
Data Protection Commission en un plazo de 3 días cuando la ley de Singapur lo exige.

## 14. Tus derechos

Puedes solicitar el acceso a tus datos, su rectificación, su supresión, la limitación de su tratamiento y su
portabilidad, y dar instrucciones sobre el destino de tus datos después de tu fallecimiento (derecho francés).

**Derecho de oposición.** Puedes oponerte en cualquier momento, por motivos relacionados con tu situación particular,
a los tratamientos basados en nuestro interés legítimo (limitación de abusos, correos, denuncias, informes de
fallos, límite de las semanas gratis).

Escribe a **support@kerfystudio.com**. Respondemos en el plazo de un mes, prorrogable dos meses para una solicitud
compleja; podemos pedirte lo necesario para verificar tu identidad.

Con tu propio servidor, como no conservamos nada que relacione un token de notificación o una denuncia con una
persona, en general no podemos encontrar esos datos sobre ti: para dejar de recibir notificaciones, desactívalas o
desinstala la app. Los datos de tu servidor los gestionas tú; para los de tu proveedor de modelos o tu proveedor de
alojamiento, dirígete a ellos.

**Con la oferta todo incluido**, tu cuenta vincula esos datos contigo: podemos darte acceso a ellos, rectificarlos o
borrarlos. Al contenido de tu plaza accedes tú mismo, por la app o por SSH, y puedes copiarlo en cualquier momento.
Para eliminar tu cuenta: sección 5.2.

**Si vives en California o en otro lugar de Estados Unidos.** Kerfy Studio no vende ni comparte tus datos personales
en el sentido de la ley californiana (California Consumer Privacy Act) y no hace ninguna publicidad dirigida. La app y
nuestros sitios no te rastrean de un sitio a otro: por eso una señal «Do Not Track» de tu navegador no cambia nada de
lo que hacemos (California Online Privacy Protection Act). Puedes ejercer los derechos de esta sección escribiendo a
support@kerfystudio.com.

## 15. Reclamación

Si consideras que no se respetan tus derechos, puedes presentar una reclamación ante la autoridad francesa de
protección de datos, la **CNIL** (3 place de Fontenoy, TSA 80715, 75334 Paris Cedex 07, Francia —
https://www.cnil.fr/fr/plaintes), o ante la autoridad de protección de datos de tu país: en el Reino Unido, la
Information Commissioner's Office (https://ico.org.uk); en Israel, la Privacy Protection Authority
(https://www.gov.il/en/departments/the_privacy_protection_authority); en Singapur, la Personal Data Protection
Commission (https://www.pdpc.gov.sg).

## 16. Menores

BipAgents está dirigida a personas **de 16 años o más**. Entre los 16 y los 18 años, úsala con el acuerdo de uno de
tus padres y respeta la edad mínima de tu proveedor de modelos (a menudo 18 años). **Antes de los 16 años, la app solo
debe usarse en una instalación preparada y supervisada por uno de los padres o un adulto a cargo del menor** (función
familia): ese adulto elige los agentes, sus accesos y el modelo, acepta las condiciones de uso por el menor y sigue
siendo responsable de su uso.

Kerfy Studio no pregunta la edad y ninguno de sus tratamientos se basa en el consentimiento. Solo el escáner de
documentos de Android pide un acuerdo, por lo que Google recibe de él (sección 7): para un menor de 15 años, el adulto
que preparó la instalación lo da junto con el menor. Si un menor nos ha escrito, uno de sus padres puede pedirnos que
borremos su mensaje.

**Oferta todo incluido**: la contrata un adulto, o se contrata con el acuerdo de uno de los padres. Una persona
invitada debe tener al menos 16 años; antes de los 16 años, una plaza solo puede usarse si uno de los padres o un
adulto a cargo del menor la ha preparado y la supervisa ([condiciones de la oferta](offer.es.md), sección 7). **En
Estados Unidos, ninguna persona menor de 13 años puede usar una plaza**, y Kerfy Studio no recoge a sabiendas ningún
dato de un menor de 13 años (Children's Online Privacy Protection Act); si ha ocurrido, escríbenos y lo borramos.

## 17. Cambios

Podemos modificar esta política, en particular antes de ofrecer enlaces de socios. La fecha al principio de la página
cambia cada vez. Un cambio importante se anuncia en la app al menos 30 días antes de aplicarse.

## 18. Contacto

Kerfy Studio — [dirección del domicilio social] — **support@kerfystudio.com** — abusos: **abuse@kerfystudio.com**

Esta política, las condiciones de uso, las [condiciones de la oferta todo incluido](offer.es.md), la
[política de uso aceptable](acceptable-use.es.md) y el aviso legal (mentions légales, en francés) también están en la
app: Ajustes › Acerca de.
