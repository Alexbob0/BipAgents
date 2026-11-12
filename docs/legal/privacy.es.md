[English](privacy.en.md) · [Français](privacy.fr.md) · [Deutsch](privacy.de.md) · Español

# Política de privacidad de BipAgents

> Última actualización: 12 de noviembre de 2026.

## En resumen

- BipAgents conecta tu teléfono con **tus propios agentes de IA**, construidos sobre Hermes Agent. Funcionan **en tu
  ordenador o tu servidor** o, con la **oferta todo incluido**, en **tu plaza**, una cuenta en un servidor que Kerfy
  Studio alquila a un proveedor de alojamiento socio. Tus conversaciones, tus archivos, tu voz y la memoria de tus
  agentes se quedan en ese servidor y en el proveedor de modelos de IA que les responde. **Kerfy Studio, el editor de
  la app, no los lee.**
- Sin publicidad, sin rastreo entre apps o sitios. Solo contamos de dónde vienen nuestros suscriptores, por campaña y
  por semana (sección 5.11), y, con la oferta todo incluido, algunas mediciones de los primeros días de una plaza, sin
  ningún contenido (sección 5.12). En Android, los servicios de Google integrados en la app (mapas, escáner de documentos)
  envían a Google sus propios datos técnicos (sección 7). Sin cuenta con tu propio servidor; una cuenta para la oferta
  todo incluido.
- Con tu propio servidor, Kerfy Studio solo puede recibir tres cosas:
  1. lo necesario para entregar tus **notificaciones** a través de su relé: un token de dispositivo sellado, nunca el
     texto de tus mensajes en claro;
  2. lo que decidas enviarnos: un **correo** al soporte o la **denuncia** de una respuesta o de un mensaje desde la app;
  3. **informes de fallos**, solo si has activado su envío en los ajustes de tu teléfono.
- Con la oferta todo incluido, Kerfy Studio trata además tu cuenta, el estado de tu suscripción, datos técnicos sobre
  tu plaza (nunca su contenido) y tus copias de seguridad, ilegibles sin su clave, que se queda en tu plaza y en tus dispositivos (sección 5.5); aloja tu plaza sin
  acceder a ella (sección 5). Si das a un agente su propia dirección de correo, Kerfy Studio guarda además los correos
  de esa dirección en su servidor de correo (sección 5.8).
- En un plan Duo o Familia, el crédito de IA es de toda la familia: **la persona que paga ve cuánto usa cada persona**
  (un importe), **nunca lo que escribe** (sección 5.3).

## 1. Quién es responsable

**Kerfy Studio**, SAS (sociedad por acciones simplificada francesa) con un capital de [capital] €, con domicilio
social en [dirección del domicilio social], inscrita en el RCS de [ciudad del registro] con el número [SIREN]
(«Kerfy Studio», «nosotros»). Contacto: **privacy@bipagents.com**.

Kerfy Studio no ha designado un delegado de protección de datos: no es obligatorio para su actividad (artículo 37 del
RGPD). Para cualquier pregunta sobre tus datos: privacy@bipagents.com. Para la ley de Singapur (Personal Data
Protection Act), que exige una persona encargada de la protección de datos, es el presidente de Kerfy Studio, a quien puedes contactar en la misma dirección.

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
- **Una dirección de correo para un agente.** En tu propio servidor, puedes dar a un agente un buzón de tu propio
  proveedor de correo. Su contraseña se queda en tu servidor; el agente solo lee los correos de las direcciones que
  has autorizado y te pide tu aprobación antes de escribir a una nueva. Se aplican las condiciones de ese proveedor, y
  Kerfy Studio no guarda nada de él.
- **Una tarjeta de pago para tus agentes.** «Pagos de tus agentes» está desactivado por defecto. En tu propio servidor,
  si lo activas y añades una tarjeta (una tarjeta virtual con límite, nunca tu tarjeta principal), su número, su fecha
  de caducidad, el nombre del titular y el código postal van a la caja fuerte cifrada de tu servidor, donde solo el
  bridge los lee, para rellenar un pago que has aprobado. El código de seguridad nunca se guarda: lo escribes en cada
  pago. El agente (el modelo de IA) nunca ve esos números, y Kerfy Studio no recibe nada. Con la oferta todo incluido,
  la tarjeta se queda en tu teléfono (sección 5.10).
- **Varias personas en un mismo servidor** (función familia): cada persona tiene su propia instalación y sus propios
  agentes, separados de los de las demás, salvo los agentes de la familia y los agentes compartidos (sección 2 bis), que
  son comunes por elección. Quien administra el servidor es responsable frente a las demás.

**Con la oferta todo incluido**, tu plaza ocupa el lugar de tu servidor, y Kerfy Studio elige el proveedor de
alojamiento y el enrutador de IA: la sección 5 dice quién ve qué.

## 2 bis. Los agentes de la familia y los agentes compartidos

**El agente de la familia.** La persona que lo crea elige quién participa: toda la familia o algunos miembros. Tiene
una sola conversación: cada uno ve en ella los mensajes de los demás, con su nombre de pila, y las respuestas del
agente. Su memoria es común: lo que tú le dices puede salir en sus respuestas a los demás. Las páginas, los audios y las
rutinas que hace llegan a esa conversación.

- **Dónde.** El agente, su conversación, su memoria, sus archivos y sus fotos están en el servidor de la persona que lo
  creó, o en su plaza con la oferta todo incluido, que Kerfy Studio aloja sin acceder a ella (sección 5.1). Los archivos
  y fotos que se le envían (fotos, PDF, Word, Excel, PowerPoint) son visibles para los miembros y se guardan según el
  plazo elegido en ese servidor, 30 días por defecto, y después se borran (sección 12).
- **Lo que nunca lee**: tu correo, tu salud y tu Cuaderno.
- **Tu calendario**, solo si se lo permites (Ajustes › Calendario, o la página del agente; desactivado al principio):
  entonces pide a tu servidor (o a tu plaza) las citas que necesita, en el momento de responder, sin guardar copia en
  casa de la persona que lo creó, y sus respuestas son visibles para los miembros. Añadir o cambiar una cita necesita
  un segundo ajuste, también desactivado al principio. Estos ajustes solo los activas tú, en tu dispositivo: ni la
  persona que creó el agente ni un padre o una madre pueden hacerlo en tu lugar, tampoco entre los 16 y los 18 años.
  Base jurídica: tu consentimiento (artículo 6.1.a del RGPD), que das al activar estos ajustes y retiras al
  desactivarlos.
- **Cuando se retira a un miembro.** Si la persona que creó el agente te retira, tus mensajes se quedan en la
  conversación común con tu nombre de pila, y tus archivos se retiran. Para dejar de formar parte del agente, pídele que
  te retire. Si eliminas tu cuenta, tu nombre de pila se sustituye por «Antiguo miembro». Puedes pedir que se borren tus
  mensajes: escribe a privacy@bipagents.com. Si la persona que creó el agente lo elimina, o elimina su cuenta o su
  plaza, el agente, su conversación y sus archivos se borran para todos.
- **Quién decide.** La persona que crea el agente elige sus miembros y sus herramientas, y puede retirar a un miembro.
  En su propio servidor, es responsable frente a los demás miembros.

**Un agente personal compartido con tus familiares.** Puedes dejarles leer tus conversaciones con uno de tus agentes
(tus mensajes, sus respuestas, las fotos, los archivos, las páginas y los mapas), desde que lo compartes, o también las
anteriores si así lo eliges (desactivado al principio). No pueden escribirle ni pedirle nada, y no ven su memoria. Lo
que el agente lee o escribe en esas conversaciones con sus herramientas (correo, calendario, contactos, Cuaderno…) pasa
a ser visible para ellos, y pueden hablar de otras personas: la app te lo recuerda antes de compartir. Si el agente
puede leer tu salud, esos pasajes no se les pueden ocultar: el uso compartido solo empieza con una casilla aparte, tu
consentimiento explícito (artículo 9.2.a del RGPD). Puedes cambiar estas elecciones o dejar de compartir cuando
quieras; una vez que dejas de compartir, ya no tienen acceso. Lo mismo vale para un agente que un familiar comparte
contigo.

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
escribir nunca nada allí, un resumen de cada día, **solo de las categorías que elegiste para ese agente**: actividad
(pasos, energía activa, minutos de ejercicio, sesiones de deporte con su tipo, duración, distancia y energía, distancia
caminando y corriendo, pisos subidos, horas de pie); sueño (tiempo dormido y en la cama, hora de acostarte y de
levantarte, fases del sueño, respiración y temperatura de la muñeca por la noche); corazón (frecuencia cardíaca en
reposo, variabilidad cardíaca, pulso medio, mínimo y máximo del día, oxígeno en sangre, VO2 máx.); cuerpo (peso, grasa
corporal, IMC, tensión arterial); comida y agua (calorías comidas, agua, cafeína); bienestar (minutos de atención
plena, estados de ánimo anotados en el iPhone); y, **desactivadas hasta que las actives una a una**, la glucosa y el
ciclo (flujo y comienzo de la regla), más sensibles. Un agente que recibe Salud empieza con actividad, sueño y corazón;
«Compartir todo» deja fuera la glucosa y el ciclo. Algunos datos solo existen en el iPhone (horas de pie, temperatura
de la muñeca, IMC, estados de ánimo). Envía los últimos 30 días la primera vez y luego los días que cambiaron (en Android, solo cuando la app
está abierta; en iPhone, también en segundo plano, como máximo una vez al día, cuando Apple Salud tiene datos nuevos),
**solo a tu servidor** (o a tu plaza con la oferta todo incluido), donde se guardan en un archivo por día, 400 días como
máximo, solo con las categorías que al menos un agente lee allí (una categoría que ya ningún agente lee se borra de
los días guardados). Cada agente solo lee sus categorías, para ayudarte con tu sueño, tu actividad, tu entrenamiento y
tus hábitos; no hacen diagnósticos. El modelo de IA del agente recibe lo que el agente lee para responderte. Con la
oferta todo incluido, solo puedes activar Salud con un modelo que funciona en la Unión Europea, y tu plaza lo comprueba
en cada lectura: si el modelo del agente ya no es uno que funciona en la Unión, el agente ya no lee nada de tu salud y
te lo dice. El teléfono solo guarda las fechas y una huella de lo que ya envió, nunca los valores, y nada en iCloud.
Kerfy Studio no los lee (con la oferta, aloja tu plaza sin acceder a ella, sección 5.1); nada se vende ni se usa para
publicidad. **Base jurídica**: tu consentimiento explícito (artículos 6.1.a y 9.2.a del RGPD). Lo das para cada agente
en la ventana «¿Compartir tu salud con …?», que dice adónde van tus datos, cuánto tiempo se guardan y qué IA los lee,
y luego al permitir las categorías en Apple Salud o Health Connect; tu teléfono guarda la fecha de ese consentimiento.
**Dejarlo**: desactiva Salud para el agente, lo que retira tu consentimiento para él (la ventana vuelve si la activas
de nuevo); cuando ya ningún agente la tiene, los días guardados en tu servidor se borran. También se borran con el
agente, con tu plaza, o al retirar el permiso en Apple Salud o Health Connect (ya no se envía nada).

**Tu agenda, si la compartes (« Calendrier », Calendario).** Para cada agente, puedes activar Calendario en sus
herramientas. Con tu permiso (Calendarios en el iPhone, Calendario en Android), la app lee las agendas de tu teléfono,
incluidas las cuentas añadidas en él (Google, Outlook…), y envía una copia de sus citas, desde una semana antes hasta
dos meses después de hoy (título, horas, lugar, notas), **solo a tu servidor** (o a tu plaza con la oferta todo
incluido), cuando se abre la app. La copia se queda allí, se sustituye en cada envío y se borra cuando ningún agente
puede leerla ya. Solo la leen los agentes que elegiste. Para cada agente, eliges si puede leer tu agenda, y si puede
añadir, mover y borrar citas (bajo la pequeña flecha de «Calendario»); al principio puede hacer ambas cosas, sin
volver a preguntarte. Cuando un agente que tiene permiso añade, cambia o borra una cita, lo hace tu teléfono; un
permiso que le retiras anula sus peticiones pendientes. El modelo de IA del agente recibe las citas que el agente lee
para responderte. Kerfy Studio no las lee (con la oferta, aloja tu plaza sin acceder a ella, sección 5.1). **Base
jurídica**: tu consentimiento (artículo 6.1.a del RGPD), que das al activar Calendario para un agente (en Android,
tras una ventana que dice qué se copia y quién lo lee) y permitirlo en tu teléfono. **Dejarlo**: desactiva Calendario
para el agente, o retira el permiso en los ajustes del teléfono (ya no se envía nada). Para un buzón que conectaste,
también puedes activar « Usar también su agenda »: tu servidor lee entonces los calendarios de esa cuenta (CalDAV, con
las credenciales del buzón, que quedan cifradas en tu servidor) cuando un agente de ese buzón los consulta, y añade o
borra las citas que pide; no se guarda nada salvo la lista de esos calendarios. Desactívalo para dejarlo.

**Tus recordatorios, si los compartes (« Rappels », Recordatorios, iPhone).** Para cada agente, puedes activar
Recordatorios en sus herramientas. Con tu permiso (Recordatorios en el iPhone), la app envía una copia de tus listas y
recordatorios, los pendientes y los marcados en los últimos 30 días (título, notas, fecha límite, lista), **solo a tu
servidor** (o a tu plaza con la oferta todo incluido), cuando se abre la app. La copia se queda allí, se sustituye en
cada envío y se borra cuando ningún agente puede leerla ya. Para cada agente, eliges si puede leerlos, y si puede
añadir y marcar recordatorios (bajo la pequeña flecha de «Recordatorios»); al principio puede hacer ambas cosas, sin
volver a preguntarte. Es entonces tu teléfono quien añade o marca el recordatorio. El modelo de IA del agente recibe
los recordatorios que el agente lee para responderte. Kerfy Studio no los lee (con la oferta, aloja tu plaza sin
acceder a ella, sección 5.1). **Base jurídica**: tu consentimiento (artículo 6.1.a del RGPD), que das al activar
Recordatorios y permitirlo en tu iPhone. **Dejarlo**: desactiva Recordatorios para el agente, o retira el permiso en
los ajustes del iPhone (ya no se envía nada). En Android, que no tiene app Recordatorios, un recordatorio a una hora
o en un día que pides a un agente va al calendario de tu teléfono (el de tu cuenta, Google Calendar por ejemplo), con
una alerta, si permites que BipAgents use el calendario; como cualquier evento de ese calendario, lo sincroniza el
proveedor de tu cuenta. La app envía entonces a tu servidor solo el título y la hora de esos recordatorios, por venir,
para que el agente sepa lo que ya pidió, y nada más de tu calendario (salvo si también compartes Calendario). Si tienes
un iPhone y un teléfono Android, estos recordatorios van al teléfono desde el que los pides (la app le dice a tu
servidor desde qué tipo de teléfono sale un mensaje, nada más). Los recordatorios por lugar
se describen más abajo.

**Tu Cuaderno (« Carnet »).** Tus agentes llevan un cuaderno con lo que les cuentas que seguirá importando (las
personas de tu vida, tus proyectos, las fechas importantes, las tareas que pides), en páginas de texto **solo en tu
servidor** (o tu plaza), compartido por todos tus agentes; lo lees, lo corriges, deshaces cambios y lo exportas desde
la app. Una vez por noche (o por semana, o nunca: lo eliges en la app), tu servidor revisa las conversaciones del día
en busca de lo que los agentes no anotaron: solo los pasajes que parecen contener esos datos se envían, una vez, a un
modelo de IA de tu proveedor de modelos, uno pequeño, por el mismo enrutador que tus agentes, para ordenarlos, con un
pequeño presupuesto mensual; las páginas se escriben en tu servidor. Los datos de salud van solo a una parte privada
que solo el agente elegido para tu salud y tú podéis leer; las conversaciones de ese agente nunca van al orden
nocturno. Kerfy Studio no recibe nada de ello. **Parar**: desactiva
« Carnet » para un agente, elige « nunca » para el orden nocturno, borra páginas en la app; todo se va con tu plaza.

**Tus contactos, si los compartes (« Contacts », Contactos).** Para cada agente, puedes activar Contactos en sus
herramientas. Con tu permiso (Contactos en el iPhone y en Android), la app lee la libreta de direcciones de tu
teléfono, incluidas las cuentas añadidas en él (iCloud, Google…), y envía una copia **solo a tu servidor** (o a tu
plaza con la oferta todo incluido), cuando se abre la app: toda la libreta o solo los grupos (etiquetas) que elijas
para ese servidor, con el nombre, apodo, empresa y cargo, las personas relacionadas y los grupos de cada contacto, y,
solo si al menos un agente de ese servidor puede verlos, sus teléfonos, direcciones de correo, direcciones postales y
cumpleaños; sus notas solo si marcas « notas » (desactivado por defecto). Las fotos nunca se envían. La copia se queda
allí y se sustituye en cada envío; lo que tu elección ya no cubre se retira enseguida, y toda la copia se borra cuando
ningún agente puede leerla ya. Solo la leen los agentes que elegiste. Para cada agente, eliges esos detalles uno por
uno, y si puede añadir y completar fichas (bajo la pequeña flecha de «Contactos»); al principio lo ve todo y puede
añadir y completar fichas. Un detalle que le retiras ya no se le muestra ni sirve para buscar; cuando ningún agente lo
ve, se retira de la copia enseguida y tu teléfono ya no lo envía. Cuando un agente que tiene permiso añade o cambia un
contacto, lo hace tu teléfono (así no se borra nada). El modelo de IA del agente recibe los contactos que el agente lee
para responderte. Kerfy Studio no los lee (con la oferta, aloja tu plaza sin acceder a ella, sección 5.1). **Tus
contactos son otras personas**: los agentes tienen la consigna de no escribirles nunca, llamarles ni pasar sus datos
sin tu permiso, y sigues siendo responsable del uso que hagas de ellos. **Base jurídica**: tu consentimiento (artículo
6.1.a del RGPD), que das al activar Contactos para un agente (en Android, tras una ventana que dice qué se copia y
quién lo lee) y permitirlo en tu teléfono. **Dejarlo**: desactiva Contactos para el agente, o retira el permiso en los
ajustes del teléfono (ya no se envía nada).

**Los recordatorios por lugar y tu ubicación.** Cuando le pides a un agente «recuérdame la leche al llegar al
súper», pide un recordatorio con el nombre del lugar (o sus coordenadas). En el iPhone, es la app Recordatorios de
Apple la que vigila el lugar: BipAgents no tiene acceso a tu ubicación; el nombre del lugar se busca en Mapas de Apple
para encontrar sus coordenadas. En Android, que no tiene app de recordatorios, BipAgents guarda él mismo estos
recordatorios: te pide el permiso de ubicación (también en segundo plano) solo cuando un agente ha creado uno, busca el
lugar con el servicio de geocodificación del teléfono y confía el círculo alrededor del lugar a los servicios de
ubicación de Google Play en el teléfono (geofencing). Tu ubicación se queda en el teléfono: no se envía ni a tu
servidor, ni al agente, ni a Kerfy Studio, y solo sirve para avisarte al llegar o al salir, incluso con la app cerrada. Los mapas de la app (sección 7) nunca
muestran tu ubicación. **Base jurídica**: tu
consentimiento (artículo 6.1.a del RGPD), que das al permitir la ubicación. **Para dejarlo**: elimina el recordatorio
(Ajustes › Recordatorios por lugar) o retira el permiso de ubicación en los ajustes del teléfono.

**Lo que compartes con la app.** Cuando compartes un mensaje o una captura de pantalla con BipAgents (desde WhatsApp,
Signal, Mensajes…), va al agente que eliges, en tu servidor, como un adjunto que envías tú; puede contener palabras de
otras personas, así que comparte solo lo que necesites. La app nunca envía el borrador de respuesta que escribe el
agente: «Enviar con…» abre la app que eliges, y lo envías tú.

**Pasar por tu teléfono para un sitio.** Algunos sitios rechazan los servidores. Para un sitio que aceptas (una vez, o
siempre, desde la tarjeta que te muestra el agente; se cambia en Ajustes), el navegador de tu agente puede pasar por la
conexión de tu teléfono: el sitio ve entonces la dirección IP de tu teléfono. La conexión sigue cifrada entre el
navegador del agente y el sitio: tu teléfono y tu servidor solo transmiten los datos y solo leen el nombre del sitio.
Una notificación (Android) o una pantalla propia (iPhone) lo muestra mientras dura, con «Detener». Nunca para un agente
compartido o familiar. Kerfy Studio no recibe nada. **Base jurídica**: prestarte la función que eliges (artículo 6.1.b
del RGPD).

**Tus elecciones de aprobación.** Las acciones que has permitido con «Permitir siempre» para cada agente, los sitios
bloqueados con «Rechazar siempre» y las solicitudes pendientes se guardan en tu servidor (o tu plaza); una solicitud pendiente se
borra tras 24 horas sin respuesta. Kerfy Studio no recibe nada.

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
- el contenido de la notificación: el nombre del agente y el principio del mensaje, **cifrados por tu servidor para la
  clave de tu teléfono**: ni nuestro relé, ni Apple, ni Google pueden leerlos. Solo pasan en claro una línea neutra, un
  código que tu teléfono muestra en tu idioma, como «Nuevo mensaje» o «Aprobación necesaria» (cualquier otro texto se
  sustituye), **identificadores técnicos** (agente, conversación, tarea, tipo de notificación) y el número que se
  muestra en el icono.

**Lo que se conserva.** El relé no tiene base de datos. El token sellado se guarda en tu teléfono y en tu servidor; con
la oferta todo incluido, el guichet de Kerfy Studio también lo guarda, siempre sellado (sección 5.6). Los registros
técnicos del relé contienen la plataforma, una huella de 4 bytes del token y la respuesta de Apple o de Google; se
conservan **6 meses como máximo**. El relé funciona, junto con el guichet de Kerfy Studio (sección 5), en un
servidor que Kerfy Studio alquila a OVHcloud (OVH SAS), un VPS en Francia, y administra él mismo; el proveedor de
alojamiento puede registrar las direcciones IP de las solicitudes, en sus registros de explotación.

**Finalidades y bases jurídicas.** Entregar las notificaciones que has activado: ejecución de las condiciones de uso
(artículo 6.1.b del RGPD). Limitar abusos y proteger el servicio: interés legítimo de Kerfy Studio (artículo 6.1.f).

**Encargados del tratamiento.** El proveedor de alojamiento del relé, OVHcloud, en Francia, y Google (Firebase Cloud
Messaging), vinculados a Kerfy Studio por un contrato conforme al artículo 28 del RGPD. Apple entrega las
notificaciones a los iPhone a través de su servicio APNs.

**Sin notificaciones.** Puedes rechazar o desactivar las notificaciones en los ajustes de tu teléfono: la app funciona
sin ellas. Si desinstalas la app, Apple o Google marcan el token como inválido y tu servidor lo olvida.

**En Android**, la app solo pide un token a Google en el momento en que activas las notificaciones.

### 4.2 El soporte y las denuncias

- **Con la oferta todo incluido**, **Ajustes › Contactar con soporte** y **Danos tu opinión** envían tu mensaje desde
  la app, a través del guichet de Kerfy Studio (sección 5), al buzón del soporte. El guichet añade tu **código de
  ayuda** (que la app te muestra), tu plan y su estado, el estado de tus plazas (número, servidor, nombre bajo
  bipagents.com, vinculada o no a tu teléfono), cómo inicias sesión (Apple, Google o correo), la versión y el idioma de
  la app y, para responderte, la dirección de correo que indiques o, si no, la de tu cuenta. **Ninguna conversación,
  ninguna clave.** El guichet guarda tu código de ayuda mientras dure la suscripción; su registro anota que se envió un
  mensaje, sin su texto.
- **Con tu propio servidor**, **Ajustes › Contactar con soporte** prepara un correo a support@bipagents.com con un diagnóstico que puedes leer
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
- **Denunciar este mensaje** (mantener pulsado el mensaje de otra persona, en un agente de la familia o un agente
  compartido, sección 2 bis) envía la denuncia desde la app, a través del relé de Kerfy Studio, y después por correo
  solo a Kerfy Studio, a través de su proveedor de correo (Proton AG, Suiza); Proton guarda el mensaje enviado hasta que lo borramos de la bandeja de salida. Contiene el motivo, tus
  precisiones opcionales, **el texto del mensaje, el nombre de pila que se muestra de su autor y el nombre del agente**.
  Ni tu nombre ni tu dirección de correo, salvo si la añades para recibir una respuesta o si la envías por correo (más
  abajo). Al autor no se le avisa de tu denuncia. Kerfy Studio no lee la conversación y no puede borrar ese mensaje.
  Puede, según el caso, aconsejarte, avisar a la persona que creó el agente si tiene cuenta, o tomar una medida prevista
  por las condiciones de uso o la política de uso aceptable: la persona afectada recibe entonces un aviso, con los
  motivos (sección 5.7). La persona que creó un agente de la familia también puede retirar a un miembro. Si estás en peligro, llama primero al 112. Ese mensaje es de otra persona: solo usamos lo que sirve para
  tramitar la denuncia. Base jurídica: el interés legítimo de Kerfy Studio en hacer respetar las reglas de la app y
  proteger a las personas que la usan (artículo 6.1.f del RGPD) y, con la oferta todo incluido, su obligación como
  prestador de alojamiento de tramitar las denuncias (artículo 6.1.c; Reglamento (UE) 2022/2065, artículo 16).
- **Si el relé no responde**, la app te propone enviar la misma denuncia por correo: entonces también recibimos tu
  dirección de correo.

El texto de la respuesta denunciada puede hablar de tu salud o de otras personas: reléelo antes de enviarlo y escribe
en las precisiones solo lo que sea útil. Borramos lo que no sirve para tramitar la denuncia.

**Finalidades.** Responderte, tramitar las denuncias, corregir errores y mejorar la app.
**Bases jurídicas.** Responder a tus mensajes y tramitar tus denuncias: interés legítimo de Kerfy Studio en
responderte y hacer la app más segura (artículo 6.1.f del RGPD). Responder a una solicitud de ejercicio de tus
derechos: obligación legal (artículo 6.1.c; artículos 12 a 22 del RGPD).
**Plazo.** Soporte: 2 años después del último intercambio; código de ayuda (oferta): mientras dure la suscripción.
Denuncia de una respuesta: 1 año después de su tramitación. Denuncia de un mensaje de otra persona: su contenido,
incluido el correo recibido en Proton, se borra en cuanto se tramita, y como muy tarde 6 meses después de recibirlo;
después solo se guarda una ficha sin el mensaje (fecha, motivo, decisión y sus motivos) durante 1 año después de la
tramitación, más tiempo si un litigio o una autoridad lo exige.
**Destinatarios.** Kerfy Studio y sus encargados del tratamiento: su proveedor de correo (Proton AG, Suiza) y el proveedor de alojamiento del relé y del guichet, por los que pasan las denuncias y los mensajes enviados desde la app.

### 4.3 Los informes de fallos

Si has activado, en los ajustes de tu teléfono, el envío de datos de análisis a los desarrolladores de apps (iPhone:
Ajustes › Privacidad y seguridad › Análisis y mejoras › Compartir con desarrolladores de apps; Android: envío de datos
de uso y diagnóstico), Apple o Google pueden transmitir a Kerfy Studio informes de fallos y mediciones técnicas
(modelo, versión del sistema, lugar del código donde la app falló). La app no tiene ninguna herramienta propia de
estadísticas ni de informes de fallos: estos informes solo vienen de Apple o de Google. En Android, los servicios de
Google integrados en la app (mapas y escáner de documentos) envían además a Google, para sí mismo, sus propios datos
técnicos y de fallos (sección 7); Kerfy Studio no los recibe.

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
  suscripción, el alojamiento de tu plaza, el envío de tus mensajes al modelo de IA, las copias de seguridad, las
  direcciones de correo de tus agentes si las activas, la supervisión técnica y la tramitación de los abusos.
- **El contenido de tu plaza** (tus agentes, tus conversaciones, tus archivos, su memoria, sus rutinas, la caja fuerte
  de contraseñas): tú decides lo que pones en ella. Kerfy Studio lo aloja y hace que lo trate el modelo de IA solo para
  prestarte el servicio (artículo 6.1.b del RGPD), **sin acceder a él**: su software no lo lee
  (las únicas órdenes que ejecuta un servidor son reinstalar una plaza, actualizarla a una versión publicada del código abierto de BipAgents o volver a abrirla, y ninguna lee su contenido). El acceso a las cuentas de Kerfy Studio en los proveedores de alojamiento está reservado a la sociedad. Solo
  se accedería al contenido de una plaza para cumplir una obligación legal (una orden judicial, por ejemplo), y se te
  informaría cuando la ley lo permita. Como todo proveedor de alojamiento, OVHcloud y Hetzner tienen acceso físico a
  sus servidores. Si tu plaza contiene datos de salud, los de Salud o lo que cuentas de tu salud a tus agentes, Kerfy
  Studio solo los aloja y los hace tratar por el modelo de IA con tu consentimiento explícito (artículo 9.2.a del
  RGPD): para Salud, el que das al activarla para un agente (sección 3), y solo con un modelo que funciona en la Unión
  Europea; para tus conversaciones, el consentimiento explícito que das al elegir hablar de tu salud con tus agentes,
  después de haber sido informado en esta política: esas palabras se alojan en tu plaza y las trata el modelo de IA
  solo para responderte, solo con un modelo que funciona en la Unión Europea, y nunca para publicidad. No estás
  obligado a hablar de ello. Puedes borrarlas al suprimir las conversaciones o tu plaza, y dejar de hablar de ello en
  cualquier momento.
- **Apple y Google** son responsables de sus propios tratamientos cuando inicias sesión con ellos y cuando pagas en su
  tienda.
- **Stripe** (Stripe Payments Europe, Ltd., Irlanda) procesa el pago cuando compras en el sitio bipagents.com
  (sección 5.3).
- **Tus agentes son IA y lo dicen**: la insignia «IA», un aviso al principio de cada sesión Live, una mención en las
  notificaciones y en los mensajes que envían a otras personas (Reglamento (UE) 2024/1689, artículo 50).

### 5.2 Tu cuenta

**Datos.** El identificador que te da Apple o Google y tu dirección de correo (con Apple, puede ser una dirección de
reenvío); o, con una dirección de correo, los códigos de un solo uso que se te envían. La lista de tus dispositivos
aprobados, la clave pública de cada uno (lo que otro de tus dispositivos le envía va sellado para él) y sus tokens de
notificación, sellados por el relé
(sección 4.1): Kerfy Studio nunca los ve en claro. Las invitaciones que envías: la dirección de correo de la persona
invitada, su estado y, solo si lo das, tu nombre, que la persona invitada ve en el correo de invitación y en su app
(una invitación por enlace guarda solo ese nombre, si se da, y su estado). Los vínculos familiares: la dirección de
correo de la otra persona, el nombre del hogar que das y una invitación sellada para cada uno de sus dispositivos, que
Kerfy Studio no puede abrir. Una conversación que compartes en lectura («Compartir en lectura», 20 personas como
máximo, hasta la fecha de fin que eliges): su copia cifrada por tu servidor (o tu plaza) antes de salir, que Kerfy
Studio guarda sin poder leerla, los nombres que escribiste para las personas invitadas y el nombre que se les muestra,
quién se unió y cuándo, tamaños y horas. Una solicitud de recuperación en curso. La
copia de tu clave de copia de seguridad, cifrada con tu código de recuperación, que Kerfy Studio no puede leer. El
crédito de IA comprado y conservado para una próxima suscripción (un importe). La fecha de eliminación de la cuenta, si
la has solicitado. Ni tu foto, nunca las claves de tu plaza.

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
n.º 2021-1362, artículos 2, 3 y 8). La huella de tu dispositivo Android (sección 5.3) se guarda aparte de tu cuenta: no se borra con ella y se queda hasta
el final de sus 24 meses, para impedir encadenar semanas gratis. Códigos de un solo uso: el tiempo de su validez.
Invitación no aceptada: 30 días; invitación de un vínculo familiar: 7 días, o hasta la respuesta. Compartir en lectura:
se borra en su fecha de fin, cuando lo detienes o con tu cuenta.
**Eliminar tu cuenta.** En la app, Ajustes › Cuenta › Eliminar mi cuenta, o, si ya no tienes la app, en la página
https://guichet.bipagents.com/account/delete con un código enviado a tu dirección de correo. Las personas con las que
compartes tu plan reciben un aviso por correo: sus plazas se detienen el día en que se borra tu cuenta.

### 5.3 Tu suscripción, tus compras y la semana gratis

**Datos recibidos de las tiendas.** A través de las notificaciones de Apple (App Store Server Notifications) y de
Google (Google Play, notificaciones en tiempo real): el identificador de tu suscripción en la tienda, el plan, la
región, su estado (semana gratis, activa, suspendida, terminada, reembolsada), el final del periodo pagado, la moneda y el precio mensual del plan, si la
renovación está desactivada y, para el crédito de IA que compras, el identificador de cada compra y su importe.
**Nunca tus medios de pago**, que solo conoce la tienda. El país de tu cuenta de la tienda, que la app envía para
saber si la oferta se vende allí (no se conserva).

**Compra en el sitio bipagents.com.** El pago pasa por **Stripe** (Stripe Payments Europe, Ltd., Irlanda). Stripe recibe
tu dirección de correo, tu dirección o tu país de facturación, los datos de tu tarjeta (que solo trata Stripe: Kerfy
Studio nunca recibe el número completo de tu tarjeta) y la dirección IP de tu dispositivo, para los controles contra el
fraude. Los trata para cobrar el pago, calcular los impuestos (Stripe Tax), prevenir el fraude y emitir las facturas.
Para sus propias obligaciones legales (lucha contra el blanqueo de capitales y el fraude), Stripe puede actuar como
responsable del tratamiento independiente, según su política de privacidad (https://stripe.com/privacy). El guichet de
Kerfy Studio guarda la dirección de correo que diste al pagar, los identificadores de cliente y de suscripción en
Stripe, el plan y su periodo (mes o año), el país de facturación y el idioma.

**Darte de baja en el sitio sin iniciar sesión** (botón «Verträge hier kündigen»): la página pide la dirección de correo
de la compra y, si quieres, el número de la suscripción, la fecha de fin que deseas y, para una baja extraordinaria, su
motivo. El guichet guarda esta declaración, con su fecha y hora, 3 años, como prueba. Para limitar los intentos,
guarda también la dirección de correo escrita, en claro, y una huella de la dirección IP (abajo), 2 días como máximo.

**Tu contrato.** La fecha de inicio, la del correo de confirmación y, si desistes del contrato, la fecha y la hora de
tu desistimiento.

**Crédito de IA usado.** El importe que la clave de IA de cada plaza ha gastado en el mes, consultado en el enrutador
de IA una vez por minuto como máximo, y el crédito traspasado: un importe, nunca tus mensajes.

**En un plan Duo o Familia**, el crédito de IA es compartido: la persona que paga ve en su app el importe que cada
plaza ha usado en el mes (y su estimación en mensajes) y el límite mensual que le haya dado. Una persona invitada ve
lo que queda para todos, lo que ha usado y su límite, nunca lo que han usado los demás. Estos importes no dicen nada
del contenido: **nadie ve lo que escribe cada uno ni lo que hacen sus agentes**. Finalidad: permitir a la persona que
paga seguir el crédito de la familia y fijar límites, y a cada persona saber lo que le queda; base jurídica: la
ejecución del contrato (artículo 6.1.b del RGPD), ya que el crédito compartido forma parte del plan Duo o Familia. El
límite fijado se conserva mientras dure la suscripción.

**Solo una semana gratis por persona.** Para saber si ya has tenido una suscripción a BipAgents, también comparamos el
correo electrónico dado en el pedido, tu cuenta BipAgents y tu identificador de cliente en Stripe con nuestras
suscripciones anteriores. No guardamos ningún dato de tu tarjeta: se introduce en Stripe y nunca pasa por nuestros
servidores.

**Solo una semana gratis por dispositivo.**

- **En iPhone**, la app usa DeviceCheck de Apple: Apple guarda dos bits por dispositivo para Kerfy Studio («ya tuvo
  una semana gratis», «ya usó un código beta»). Kerfy Studio no recibe ningún identificador del dispositivo: la app le da un token de un solo
  uso que transmite a Apple.
- **En Android**, la app envía un token de Google Play Integrity (Google confirma que la app es auténtica y funciona en
  un dispositivo real) y una huella del identificador Android del dispositivo, calculada en el teléfono (SHA-256) y
  después vuelta a calcular por el guichet con una clave secreta antes de guardarla. Kerfy Studio solo conserva esta
  segunda huella, solo para este límite y para el de los códigos beta (abajo).
- **La dirección IP** de tu teléfono nunca se guarda en claro. Para limitar el número de reservas por dirección, el
  guichet solo conserva una huella calculada con una clave secreta, únicamente en memoria, mientras dura la reserva de
  una plaza de prueba (15 minutos como máximo); se pierde al reiniciarse. Para limitar los intentos de códigos beta, en
  la página de baja del sitio y en las denuncias de agentes compartidos como plantilla, guarda del mismo modo una
  huella, nunca la dirección, 2 días como máximo.

**Los códigos beta.** Un código beta usado se convierte en el identificador de tu suscripción beta, que se guarda con
tu cuenta como cualquier suscripción (abajo), y el registro del guichet anota «código usado» con tu cuenta (12 meses).
Un solo código por teléfono: en iPhone, Apple guarda para ello el segundo bit de DeviceCheck; en Android, el guichet
guarda una huella del dispositivo, aparte de tu código y de tu cuenta, mientras aún se puedan usar
códigos. Un código nunca usado se borra en cuanto ya no puede usarse; un código usado, con su vínculo a tu cuenta,
cuando tu plaza beta termina o pasa a una suscripción de pago.

**Finalidades y bases jurídicas.** Prestar y gestionar tu suscripción, tus plazas y tu crédito: ejecución del contrato
(artículo 6.1.b del RGPD). Evitar que una misma persona encadene semanas gratis: interés legítimo de Kerfy Studio en
prevenir el fraude y en poder seguir ofreciendo una semana gratis (artículo 6.1.f y considerando 47). Leer el
identificador del dispositivo no requiere tu consentimiento, porque solo sirve para limitar el acceso gratuito al
servicio a un periodo limitado (artículo 82 de la ley francesa de protección de datos; CNIL, deliberación n.º 2020-091,
artículo 5). Conservar la prueba del contrato: interés legítimo de Kerfy Studio en poder defender sus derechos. Compra
en el sitio: cobrar el pago y calcular los impuestos, ejecución del contrato (artículo 6.1.b); controles contra el
fraude, interés legítimo (artículo 6.1.f); facturas y documentos contables, obligación legal (artículo 6.1.c).
**Plazo.** Datos de suscripción y de compra, incluidos los de una compra en el sitio: durante la suscripción y después
5 años archivados, como prueba del contrato (artículo 2224 del Código Civil francés). Facturas y justificantes de pago:
10 años (artículo L123-22 del Código de Comercio francés); Stripe guarda las facturas. Huella del dispositivo Android:
24 meses desde la primera marca del teléfono (su semana gratis, o la primera suscripción hecha desde él), guardada
aparte de tu cuenta. Dirección IP: nunca se guarda en claro; su huella, en memoria, 15 minutos como
máximo para la semana gratis, 2 días como máximo para los demás límites. La huella guardada 2 días, la de un teléfono Android que usó un código beta y las mediciones de cada plaza (sección 5.12) quedan después hasta 30 días en las copias cifradas de la base. Los bits de DeviceCheck los guarda Apple.

### 5.4 Tu plaza y el modelo de IA

**Dónde.** En un servidor alquilado a OVHcloud (centros de datos en la Unión Europea) o a Hetzner (Alemania o
Finlandia), para todas las personas de la oferta, también las que viven fuera de la Unión.

**Lo que hay.** Lo que describe la sección 5.1, y las claves SSH públicas que añades desde la app. El guichet de Kerfy
Studio (el servicio que gestiona las plazas) conoce la dirección de tu plaza y la huella de su certificado, para que tu
teléfono se conecte a ella y para trasladarla en caso de avería.

**El modelo de IA.** Los mensajes que tus agentes envían al modelo (con los archivos y las fotos que le muestran)
pasan por un enrutador de IA, **EUrouter** (EUrouter B.V., Ámsterdam, Países Bajos), para todas las personas de la
oferta, y después por el proveedor de alojamiento que hace funcionar el modelo. Hoy, la oferta usa un solo modelo,
GLM-5.3 Flash (fabricante: Z.ai); si se retirara, el modelo de sustitución previsto es DeepSeek V4.1 Flash
(fabricante: DeepSeek). EUrouter indica que los hace funcionar solo en sus proveedores de alojamiento asociados
establecidos en la Unión Europea, en servidores de la Unión; los fabricantes de esos modelos no reciben tus mensajes.
No ofreceremos otro modelo en la oferta sin actualizar antes esta sección. EUrouter indica que por defecto no guarda ni las solicitudes ni las respuestas, y
que guarda sus metadatos (hora, modelo, número de tokens, coste, dirección IP) hasta 12 meses. En cada solicitud de la
oferta, tu plaza pide además a EUrouter que **no se entrene** ningún modelo con tus mensajes y que **no se guarden**:
EUrouter solo la confía entonces a un proveedor que, según EUrouter, cumple ambas reglas. Actúan por cuenta de
Kerfy Studio. **Antes del primer mensaje, la app nombra el enrutador y el fabricante del modelo y te pide tu
acuerdo**. **Para Salud**, solo un modelo que funciona en la Unión
Europea: tu plaza se niega a dar tus datos de salud a un agente cuyo modelo no está marcado así, aunque el modelo haya
cambiado después de que activaras Salud (sección 3).

**El reposo.** Tu plaza anota ella misma la última solicitud de la app; el guichet sabe que duerme (sección 5.6).

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).
**Plazo.** Durante la suscripción. Cuando termina, tu plaza se conserva 30 días (3 días después de una semana gratis
que no continuó) y después su cuenta se borra y se reinstala para otra persona. Las plazas de más de un plan reducido
se borran 30 días después de detenerse.

### 5.5 Las copias de seguridad

Tu plaza hace una copia de seguridad unos minutos después de cada actividad, y al menos una vez al día. La copia se **cifra en la plaza**, antes de salir, con una clave que
se queda en la plaza y en tus dispositivos, más una copia de esa clave cifrada con tu código de recuperación (sección
5.2), que Kerfy Studio no puede abrir. En la oferta todo incluido, la plaza está en un servidor que alquila Kerfy
Studio: la clave también está allí y,
técnicamente, el proveedor y Kerfy Studio podrían acceder a ella; nuestro software no lee ni tu plaza ni sus copias, y
solo lo haríamos si la ley nos obligara. Kerfy Studio guarda estas copias en **Scaleway** (Scaleway SAS, Francia;
almacenamiento de objetos en París, región fr-par) sin poder leerlas: las de las 2 últimas horas, luego una por hora durante 48 horas, una por día durante
21 días, nada más antiguo. Se borran con tu plaza. Lo que una plaza borra de sus copias, el guichet lo aparta primero 5 días (contra un borrado malintencionado) y
luego se borra; en Scaleway, una versión sustituida o borrada de un archivo se guarda 1 día y luego se borra. Así, lo que borras o pides a un agente que olvide ha salido de las copias de seguridad, todas sus copias incluidas, **en 30 días**. Los datos se quedan en la Unión Europea.

**Las copias de seguridad del guichet.** El guichet (tu cuenta, tu suscripción, los datos técnicos de las plazas)
también se guarda en Scaleway, en copias que Kerfy Studio cifra antes de enviarlas; las versiones antiguas se borran
a los 30 días.

**La imagen diaria de OVHcloud.** En sus servidores, OVHcloud hace además una imagen diaria del servidor entero, que
guarda en sus instalaciones: un punto de restauración por día, no cifrado por Kerfy Studio. Cuando una plaza se
reinstala para otra persona, la imagen que la contenía se sustituye en un día aproximadamente.

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).

### 5.6 La supervisión técnica y las averías

Cada dos minutos, cada servidor envía al guichet: su memoria, su disco y su carga y, para cada plaza, si su bridge
responde, si su Hermes Agent funciona o duerme, su versión de BipAgents, la hora de su última copia de seguridad
correcta, la memoria que usa, si hay una tarea en curso, el número de sus agentes que tienen su propio espacio y tu
zona horaria (la hora de su última solicitud de la app y de su próxima rutina también se envían, pero el guichet no
las guarda). **Nunca contenido.** Estos datos sirven para detectar una avería, trasladar las plazas (durante tu noche,
nunca en plena tarea), hacer las actualizaciones por oleadas y avisar a Kerfy Studio.

Cuando el servidor de tu plaza se avería, se traslada o vuelve, tus teléfonos reciben un aviso a través del relé de
notificaciones, con un texto genérico: para ello, el guichet guarda los tokens sellados de tus teléfonos vinculados a
tu plaza.

**Base jurídica.** Ejecución del contrato (artículo 6.1.b del RGPD).
**Plazo.** Los últimos valores, sustituidos en cada envío, salvo la memoria de cada plaza: su valor más alto de cada
hora, durante 7 días (168 valores), para repartir las plazas entre los servidores. Registro de eventos (averías,
traslados, actualizaciones, eventos de tu cuenta y de tu suscripción con sus identificadores, nunca contenido):
12 meses. Tokens sellados: mientras el dispositivo esté vinculado a tu plaza.

### 5.7 Las denuncias de abusos y las solicitudes de las autoridades

**Datos.** La denuncia (quién la envía y cómo contactarle, los hechos, la dirección IP y la hora), la plaza afectada,
nuestra decisión y sus motivos, nuestros intercambios contigo.
**Finalidades.** Tramitar las denuncias ([política de uso aceptable](acceptable-use.es.md)), proteger los servidores y
a las demás personas, responder a las autoridades.
**Bases jurídicas.** Obligación legal (artículo 6.1.c del RGPD; Reglamento (UE) 2022/2065, artículos 16 a 18) e interés
legítimo de Kerfy Studio en proteger su servicio y a sus clientes (artículo 6.1.f).
**Plazo.** 1 año después de su tramitación, más tiempo si un litigio o un procedimiento lo exige.

### 5.8 Las direcciones de correo de tus agentes

**Qué es.** Puedes dar a un agente que elijas su propia dirección de correo, por ejemplo `lea.k7m2@a.bipagents.com`.
Está desactivada por defecto: la activas para ese agente en la app y puedes quitarla en cualquier momento.

**Dónde.** El buzón se guarda en el servidor de correo propio de Kerfy Studio, un servidor dedicado alquilado a
Hetzner (Alemania o Finlandia), aparte de los servidores de las plazas.

**Datos.** La propia dirección, que el guichet vincula a tu plaza. Los correos recibidos y enviados por esa dirección:
sus cabeceras (remitentes, destinatarios, fechas, asuntos), su contenido y sus adjuntos. La contraseña de la dirección
se crea en el servidor de Kerfy Studio y se entrega una sola vez a tu plaza, donde se guarda con los ajustes del
agente; el servidor de correo solo guarda un hash de ella, y el guichet no la guarda. Las direcciones a las que tu
agente puede escribir, su límite diario y los correos que esperan tu aprobación se guardan en tu plaza.

**Quién puede escribir a tu agente.** El agente solo lee los correos de las direcciones que has autorizado, cuando su
remitente está verificado (las comprobaciones habituales contra remitentes falsificados). No lee ni responde los
correos de nadie más: se quedan en el buzón hasta que se borran con los demás (más abajo). Un correo lo escribe otra
persona: el agente toma lo que dice como información, nunca como instrucciones que seguir.

**Lo que tu agente puede enviar.** Responde a las direcciones que has autorizado. Para escribir a una dirección nueva,
necesita tu aprobación en la app: el correo espera hasta que lo apruebes (la dirección queda entonces autorizada) o lo
descartes. Envía como máximo el número de correos al día que fijas en la app (20 por defecto).

**Spam y seguridad.** El servidor de correo filtra el spam y guarda registros de seguridad (las direcciones IP de los
servidores que envían, la entrega de cada correo, las conexiones al buzón) durante **30 días**, para entregar los
correos, bloquear el spam y los abusos e investigar un incidente.

**Acceso.** Como con el contenido de tu plaza (sección 5.1), Kerfy Studio no lee estos correos: su software solo los
entrega, filtra el spam y los borra cuando se cumple su plazo. Entre servidores de correo, los correos van cifrados
durante el trayecto cuando el otro servidor lo permite; un correo no está cifrado de extremo a extremo.

**Finalidades y bases jurídicas.** Dar a tu agente una dirección para recibir los correos de las personas que
autorizas y responderles: ejecución del contrato (artículo 6.1.b del RGPD). Filtrado del spam y registros de
seguridad: interés legítimo de Kerfy Studio en proteger su servicio y a las personas que lo usan (artículo 6.1.f).

**Plazo.** Los correos recibidos y enviados se borran a los **90 días**. Este plazo es un ajuste de la oferta: puede
acortarse, y nunca se alarga sin avisarte antes. El buzón entero se borra de una vez cuando quitas la dirección o el
agente, cuando se borra tu plaza (sección 5.4) y cuando se borra tu cuenta (sección 5.2). Registros de seguridad: 30
días.

**Tu propio buzón en su lugar.** En tu propio servidor, puedes conectar en su lugar un buzón de tu propio proveedor de
correo (sección 2): sus credenciales se quedan en tu servidor, se aplican las condiciones de ese proveedor, BipAgents
no borra nada en él y Kerfy Studio no guarda nada de él.

### 5.9 Los prestadores de la oferta

| Prestador | Papel | Datos | Dónde |
|---|---|---|---|
| OVHcloud (OVH SAS, Roubaix, Francia) | Aloja los servidores de las plazas; imagen diaria de cada servidor | Contenido de las plazas | Unión Europea |
| Hetzner Online GmbH (Gunzenhausen, Alemania) | Aloja parte de los servidores de las plazas y el servidor de correo de los agentes | Contenido de las plazas; buzones de los agentes y registros de seguridad del servidor de correo | Alemania, Finlandia |
| EUrouter B.V. (Ámsterdam, Países Bajos) | Enrutador de IA | Mensajes enviados al modelo, metadatos | Unión Europea |
| Proveedores de alojamiento de los modelos, socios de EUrouter | Hacen funcionar el modelo (hoy GLM-5.3 Flash) | Mensajes enviados al modelo, con los archivos y las fotos que ve | Unión Europea, según EUrouter |
| Scaleway SAS (París, Francia) | Almacenamiento de objetos de las copias de seguridad (región fr-par, París) | Copias de seguridad cifradas de las plazas, ilegibles para él y para Kerfy Studio; copias cifradas de la base de datos del guichet; una versión sustituida o borrada se guarda 1 día | Unión Europea (Francia) |
| OVHcloud (OVH SAS, Roubaix, Francia) | Aloja el guichet y el relé, en un VPS en Francia | Cuenta, suscripción, datos técnicos; tokens sellados y los registros del relé (secciones 4.1 y 5.6) | Francia |
| Proton AG (Ginebra, Suiza) | Envía, desde una dirección bipagents.com, los códigos, los avisos de seguridad, las invitaciones, la confirmación de compra, el acuse de desistimiento y los avisos de eliminación; también aloja el buzón de soporte (sección 4.2) | Dirección de correo, contenido de esos correos; Proton guarda el mensaje enviado hasta que lo borramos de la bandeja de salida | Suiza (decisión de adecuación, sección 11) |
| Stripe Payments Europe, Ltd. (Dublín, Irlanda) | Pago de las compras en el sitio bipagents.com, cálculo de impuestos, controles contra el fraude, facturas | Dirección de correo, dirección o país de facturación, datos de tarjeta, dirección IP | Unión Europea; posiblemente fuera de la Unión (sección 11) |

Están vinculados a Kerfy Studio por un contrato conforme al artículo 28 del RGPD; Stripe actúa además como responsable
independiente para sus propias obligaciones legales (sección 5.3). Apple (DeviceCheck, App Store) y Google (Play
Integrity, Google Play) también intervienen, como dice la sección 5.3; «Iniciar sesión con Apple» y el inicio de sesión
con Google son de su propia responsabilidad.

### 5.10 Los pagos de tus agentes (experimental, durante la beta)

Por defecto tus agentes no pagan nada: preparan una cesta y pagas tú («Cesta lista»). Si Kerfy Studio ha abierto la
función durante la beta y **tú la activas**, un agente puede pagar una pequeña compra con una tarjeta que le confíes,
en los sitios que permitas:

- El número, la fecha de caducidad, el nombre del titular y el código postal de la tarjeta se quedan **en el teléfono
  donde la añadiste**, en su almacenamiento protegido (Llavero de iOS, Keystore de Android), tras Face ID, tu huella o
  tu código. Tu plaza solo guarda el nombre que le diste a la tarjeta, su red, sus 4 últimos dígitos, el límite que
  fijaste, los sitios y agentes permitidos y los pagos hechos con ella (importe, sitio, agente, fecha).
- **En cada pago que apruebas** en la app, tu teléfono envía estos datos y el código de seguridad que acabas de
  escribir a tu plaza, por la conexión cifrada de la app. El software de tu plaza los guarda en memoria el tiempo de
  rellenar el formulario de pago en el navegador del agente y luego los borra. Nunca se escriben en el disco, en los
  registros ni en las copias de seguridad, y el agente (el modelo de IA) no los ve.
- El registro de pagos (sin ningún número de tarjeta) se guarda 3 años en tu plaza.
- Finalidad: hacer la compra que pediste y aprobaste (ejecución del contrato, artículo 6.1.b del RGPD). Puedes
  suspenderlo todo en cualquier momento y borrar la tarjeta en la app (se borra entonces del teléfono y de tu plaza).

Es una función experimental: algunos bancos consideran que confiar los datos de la tarjeta a un servicio es una
negligencia y pueden negarse a reembolsar en caso de disputa. Por eso la app pide una tarjeta virtual con límite,
nunca tu tarjeta principal.

### 5.11 Cómo nos conociste (medición de nuestras campañas)

Para saber qué campañas nos traen suscriptores, y detener las demás, tu cuenta y tu suscripción guardan como máximo
**un nombre de campaña** (una etiqueta corta, por ejemplo «insta-nov-a»), su fuente y su fecha. Viene de una sola de
estas fuentes:

- en Android, el nombre de campaña del enlace de Google Play que te hizo instalar la app (solo ese nombre), leído una
  vez en el primer arranque;
- en iPhone, el número de la campaña de Apple Search Ads: durante los primeros 30 días, la app pide a Apple un token de
  atribución (AdServices), que el guichet canjea ante Apple por ese número, sin guardar el token;
- el enlace del sitio por el que te suscribiste, o el lote de tu código beta;
- tu respuesta a «¿Cómo nos conociste?», si respondes.

Se guarda el primer nombre recibido, nunca se sustituye. La app lo envía una sola vez, y solo si tienes una cuenta o la
oferta todo incluido: nada para una instalación en tu propio servidor sin cuenta. Nunca un identificador publicitario
ni de clic, ninguna herramienta publicitaria en la app, nada enviado a Meta ni a ningún anunciante.

**Uso.** Solo un recuento por campaña y por semana, que nunca contiene a una persona. Nunca para escribirte, hacerte
una oferta o perfilarte, y el nombre no se transmite a nadie. **Base jurídica.** Interés legítimo de Kerfy Studio en
saber qué campañas son útiles (artículo 6.1.f del RGPD); leer el enlace de Google Play y pedir el token de Apple en tu
teléfono solo sirve para esta medición de audiencia. **Plazo.** 13 meses después de su registro, o menos si tu cuenta o
tu suscripción se borra antes. **Oponerte.** Escribe a privacy@bipagents.com: borramos el tuyo y no registramos más
para tu cuenta y tus suscripciones.

### 5.12 Las mediciones de los primeros días de una plaza

Para saber si los primeros pasos en la app van bien, tu plaza cuenta algunos gestos, **sin ningún contenido** (ni
mensaje, ni nombre, ni agente): la hora de tu primer mensaje al asistente que te ayuda a prepararlo todo, de la
primera de sus propuestas que aceptas y de la primera rutina recibida, el número de equipos propuestos, aceptados o
rechazados y de agentes creados así, y las horas en que escribiste (redondeadas a la hora). Las envía al guichet con
la supervisión técnica (sección 5.6). El guichet las guarda **por plaza** (y por tanto vinculadas a tu suscripción),
sin las horas: solo si escribiste al día siguiente y una semana después. Esto vale para todas las plazas de la oferta
(código beta, semana gratis, suscripción de pago). Con tu propio servidor, nada de esto sale.

**Base jurídica.** Interés legítimo de Kerfy Studio en mejorar los primeros pasos en la app (artículo 6.1.f del RGPD).
**Plazo.** Por plaza, durante sus primeros días (11 días como máximo después de que se te diera la plaza), y luego
solo sumadas a los totales del día, en los que no aparece nadie, guardados 2 años. Borradas en el acto cuando la plaza
se reinstala, se retira o se da a otra persona.

### 5.13 Pedir un código beta en el sitio

En bipagents.com, «Obtener un código» te pide tu dirección de correo y hace que tu navegador haga un pequeño cálculo
contra los robots (ALTCHA, servido y verificado por Kerfy Studio, que no deja nada en tu dispositivo: ni cookie, ni
almacenamiento). Cada dirección recibe un solo código, las direcciones desechables se rechazan, y el número de códigos
está limitado por día y por red. Tu dirección se lee un instante, solo para enviarte tu código a través de Proton AG
(Suiza, sección 11), desde una dirección bipagents.com. No la guardamos, y Proton guarda el mensaje enviado hasta que lo borramos de la bandeja de salida. Solo guardamos una huella de tu
dirección, calculada con una clave secreta, para comprobar que una dirección recibe un solo código: es un dato
seudonimizado, y por tanto sigue siendo un dato personal. Tu dirección IP nunca se guarda en claro: solo su huella sirve
para el límite por red, 2 días como máximo (sección 5.3).

**Bases jurídicas.** Enviarte el código que pides (artículo 6.1.b del RGPD). Un solo código por dirección, el límite por
red y el cálculo contra los robots: interés legítimo de Kerfy Studio en prevenir los abusos y repartir de forma justa las
plazas de la beta (artículo 6.1.f). **Plazo.** La huella de la dirección, hasta el final de la beta, como muy tarde el 31 de diciembre de
2026. Si después creas una cuenta, la sección 5.2 se aplica a tu cuenta.

## 6. Lo que Kerfy Studio no hace

- Sin publicidad, sin identificador publicitario, sin rastreo entre apps o sitios. Ninguna herramienta de estadísticas
  en la app: solo el recuento de nuestras campañas (sección 5.11) y, con la oferta, las mediciones de los primeros
  días de una plaza (sección 5.12). Sin cuenta, salvo para la oferta todo incluido.
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
- **Tiempo e itinerarios.** Cuando un agente tiene « Tiempo » y le preguntas qué tiempo hará, tu servidor (o tu
  plaza) pide el lugar a OpenStreetMap (Nominatim, para encontrar la ciudad) y luego el pronóstico al Instituto
  Meteorológico Noruego (MET Norway): reciben **solo el nombre del lugar y sus coordenadas redondeadas** (unos 1 km), y
  ven la dirección de tu servidor, no la de tu teléfono ni tu mensaje. Un itinerario es un enlace que abre Mapas
  (iPhone) o Google Maps (Android) en tu teléfono, con sus propias reglas. Si tu servidor tiene una clave de Google
  Maps (de momento no en la oferta todo incluido), los lugares y la hora del itinerario también van a Google (Google
  LLC, posiblemente en Estados Unidos, certificada en el EU-U.S. Data Privacy Framework) para calcular su duración; no
  se guarda nada.
- **Los mapas.** Los mapas de las páginas de tus agentes vienen de tu servidor (o de tu plaza), que obtiene sus piezas
  de OpenFreeMap: tu teléfono no contacta con OpenFreeMap. Para una conversación que te comparten en lectura, tu
  teléfono los pide él mismo a OpenFreeMap, que ve entonces su dirección IP y la zona del mapa. Los mapas de las
  respuestas (lugares, itinerarios) son los de Mapas de Apple (MapKit) en iPhone y del SDK de Google Maps en Android.
  Cuando uno de esos mapas aparece en Android, Google recibe información sobre el dispositivo y la solicitud, la
  dirección IP, un identificador propio del SDK, los informes de fallos del SDK y, cuando el mapa se mueve o se amplía,
  esos eventos; Google indica que los usa para hacer funcionar y mejorar sus servicios. Estos mapas muestran los
  lugares de la respuesta, nunca tu ubicación. Google LLC puede tratar estos datos en Estados Unidos. Para situar los
  lugares de una respuesta en estos mapas, tu teléfono envía sus nombres a Mapas de Apple (iPhone) o al servicio de
  geocodificación del teléfono, en general el de Google (Android). Para dibujar un mapa o un itinerario en una
  respuesta o una página, tu servidor (o tu plaza) pide además los lugares a OpenStreetMap (Nominatim, que recibe los
  nombres o direcciones de los lugares), las imágenes del mapa a OpenStreetMap (que recibe la zona del mapa) y el
  camino por las calles a FOSSGIS (Alemania, que recibe las coordenadas de las etapas): ven la dirección de tu
  servidor, no la de tu teléfono ni tu mensaje.
- **Reconocimiento de voz.** Las notas de voz y Live usan el reconocimiento de voz de tu teléfono. La app pide el
  reconocimiento en el dispositivo cuando tu teléfono lo ofrece para tu idioma. Si no, Apple o Google (en Android, el
  servicio de reconocimiento del sistema, a menudo el de Google) tratan el audio en sus servidores, bajo su propia
  responsabilidad. Lo que va a tu servidor es el texto transcrito. Base jurídica: ofrecerte la función que usas
  (artículo 6.1.b del RGPD).
- **Síntesis de voz del teléfono.** Cuando la voz del teléfono lee un texto (en la demo, o si tu servidor no
  responde), usa el motor de síntesis de tu teléfono, según sus ajustes.
- **En Android, los códigos QR** se leen en el teléfono, con una biblioteca integrada en la app, sin ningún servicio de
  Google.
- **El escáner de documentos y la lectura del texto.** El reconocimiento del texto de los documentos escaneados se
  hace en tu teléfono: en iPhone con Vision de Apple, en Android con ML Kit de Google, que proporcionan los servicios de
  Google Play, que descargan su modelo. El texto no se envía ni a Apple ni a Google. En Android, el escáner también es
  una función de ML Kit, y Google indica que ML Kit le envía, cifrada, información sobre el dispositivo (fabricante,
  modelo, versión del sistema) y sobre la app (nombre y versión), mediciones de rendimiento (como la latencia) y la
  configuración de la API (como el formato y la resolución de la imagen), para el diagnóstico y estadísticas de uso,
  sin transmitirla a terceros. La app te lo dice y te pide tu acuerdo antes del primer uso del escáner, y ese acuerdo
  cubre también la lectura del texto; sin él, el escáner no se abre y ML Kit no lee ningún documento, y siempre puedes
  adjuntar una foto o un archivo. Puedes cambiar de opinión en cualquier momento en Ajustes.
- **Apple y Google** para las notificaciones (sección 4.1).
- **Tailscale**, si lo usas: una app distinta, con su propia cuenta y su propia política de privacidad.
- **Los enlaces que abres** (sitios citados por tus agentes, página de Hermes Agent) se abren en tu navegador. Tu
  teléfono nunca abre por sí solo un enlace escrito por un agente: solo un enlace de Google Maps se convierte en un
  pequeño mapa, hecho a partir del propio enlace, como los mapas de las respuestas (arriba).

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

El pequeño modelo que ayuda al Live a saber cuándo has terminado de hablar (unos 35 MB) lo descarga tu teléfono, por
wifi, desde GitHub (GitHub, Inc., filial de Microsoft), mientras no esté en el teléfono: GitHub ve la dirección IP de
tu teléfono, nada más.

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

El relé de notificaciones está alojado en Francia, en OVHcloud (OVH SAS) (sección 4.1). Para entregar las
notificaciones, Apple y Google pueden tratar el token y la notificación fuera de la Unión Europea, en particular en
Estados Unidos:

- Google LLC (Firebase Cloud Messaging) está certificada en el «EU-U.S. Data Privacy Framework», reconocido como
  garante de una protección adecuada por la Decisión de Ejecución (UE) 2023/1795 de la Comisión, de 10 de julio de
  2023;
- Apple indica que enmarca las transferencias desde el Espacio Económico Europeo con las cláusulas contractuales tipo
  adoptadas por la Comisión Europea.

**La oferta todo incluido.** Los servidores de las plazas, el servidor de correo de los agentes y las copias de
seguridad (Scaleway, Francia) están en la Unión Europea, para todas las personas de la oferta. EUrouter trata tus mensajes en la Unión
Europea, e indica que hace funcionar los modelos de la oferta solo en proveedores de alojamiento de la Unión
(sección 5.4). Si un modelo de la oferta llegara a funcionar fuera de la Unión, esta sección diría dónde y con qué
garantía antes de ofrecerlo. Apple y
Google (inicio de sesión, tiendas, DeviceCheck, Play Integrity) pueden tratar datos en Estados Unidos, con las garantías
indicadas arriba. Para una compra en el sitio, Stripe puede tratar datos fuera de la Unión Europea, en particular en
Estados Unidos, con las garantías que describe su política de privacidad (https://stripe.com/privacy).

**Si vives fuera de la Unión Europea** (Estados Unidos, Reino Unido, Israel, Singapur), tus datos vienen a la Unión
Europea, donde Kerfy Studio los trata conforme al RGPD, que se aplica a todos sus tratamientos. El Reino Unido e Israel
reconocen que la Unión Europea ofrece una protección adecuada; para Singapur, ese nivel de protección y nuestros
contratos con nuestros prestadores dan la protección comparable que exige la ley de Singapur.

El modelo del Live se descarga desde GitHub (GitHub, Inc., Estados Unidos), certificada en el «EU-U.S. Data Privacy
Framework» (sección 9). Las imágenes de los mapas y Nominatim los sirve la Fundación OpenStreetMap, en el Reino Unido,
país que la Comisión Europea reconoce como proveedor de una protección adecuada (sección 7).

Proveedor de correo, que también envía los correos del guichet (códigos, avisos de seguridad, invitaciones,
confirmaciones, avisos de eliminación): Proton AG, en Suiza, país que la Comisión Europea reconoce como con un nivel
de protección adecuado (Decisión 2000/518/CE, mantenida en vigor por el artículo 45, apartado 9, del RGPD). Proveedor
de alojamiento del guichet y del relé: OVHcloud, en Francia. Proveedor de alojamiento de las copias de seguridad:
Scaleway, en Francia, sin transferencia fuera de la Unión. Puedes pedirnos una copia de las garantías aplicables.

## 12. Plazos de conservación

| Datos | Plazo |
|---|---|
| Datos de la app en tu teléfono | Hasta que elimines el agente o la app |
| Datos de salud (« Santé ») en tu servidor o tu plaza | 400 días como máximo; se borran cuando ningún agente tiene Salud, con el agente o con la plaza |
| Tu consentimiento para Salud (y, en Android, para los contactos y la agenda), con su fecha, en tu teléfono | Hasta que borres la app; marcado como retirado, con su fecha, cuando desactivas la herramienta para el agente |
| Copia de la agenda, los recordatorios y los contactos en tu servidor o tu plaza | Sustituida cada vez que se abre la app; borrada cuando ningún agente puede leerla (también cuando se borra el último) o con la plaza |
| Peticiones de los agentes a tu teléfono (citas, recordatorios, contactos) | Hasta que tu teléfono las haga o se anulen, y luego borradas en la copia siguiente una vez cumplidos 14 días; como tarde con la plaza |
| Token de dispositivo en el relé | Ninguna conservación: solo existe sellado, en tu teléfono y tu servidor (y en el guichet con la oferta todo incluido, mientras el dispositivo esté vinculado a tu plaza) |
| Dirección IP en el relé | En memoria, una hora como máximo, en forma de huella |
| Registros técnicos del relé | 6 meses como máximo |
| Correos de soporte | 2 años después del último intercambio |
| Código de ayuda (oferta todo incluido) | Mientras dure la suscripción |
| Denuncia de una respuesta | 1 año después de su tramitación |
| Denuncia de un mensaje de otra persona | Contenido: hasta su tramitación, 6 meses como máximo después de recibirlo; ficha sin el mensaje: 1 año después de la tramitación (más si un litigio o una autoridad lo exige) |
| Solicitud de aprobación pendiente, en tu servidor o tu plaza | 24 horas sin respuesta, y después se borra |
| Informes de fallos | 1 año después de su recepción (en Kerfy Studio) |
| Cuenta de la oferta (identificadores, correo, dispositivos, invitaciones, crédito conservado) | Mientras exista la cuenta; se borra 30 días después de una solicitud de eliminación |
| Invitación no aceptada | 30 días |
| Dirección de correo e identificador de cuenta después del fin del contrato (obligación de los proveedores de alojamiento) | 5 años para el correo, 1 año para el identificador, conservados aparte |
| Suscripción y compras | Durante la suscripción y después 5 años archivados |
| Compra en el sitio (dirección de correo dada al pagar, identificadores de Stripe, plan y periodo, país de facturación, idioma) | Durante la suscripción y después 5 años archivados |
| Facturas y justificantes de pago de una compra en el sitio | 10 años (artículo L123-22 del Código de Comercio francés); facturas guardadas por Stripe |
| Código beta nunca usado | Hasta que ya no pueda usarse |
| Código beta usado y su vínculo a tu cuenta | Hasta el final de tu plaza beta o su paso a una suscripción de pago |
| Huella de un teléfono Android que usó un código beta (guardada aparte del código y de la cuenta) | Mientras aún se puedan usar códigos beta; luego hasta 30 días en las copias cifradas de la base |
| Huella del dispositivo Android, calculada con una clave secreta (semana gratis) | 24 meses desde la primera marca del teléfono; guardada aparte de la cuenta, no se borra con ella |
| Invitación de un vínculo familiar | 7 días, o hasta la respuesta |
| Compartir en lectura (copia cifrada, personas invitadas) | Hasta su fecha de fin, hasta que se detiene o hasta que se borra la cuenta |
| Conversación y memoria de un agente de la familia | Mientras el agente exista en casa de la persona que lo creó |
| Archivos enviados al agente de la familia | En el servidor de la persona que lo creó, hasta que quien los envió los elimina, deja de usar el agente, es retirado de él o deja la familia, o el agente se elimina; como muy tarde, el plazo elegido allí para los archivos (30 días por defecto) |
| Nombre de campaña (sección 5.11) | 13 meses después de su registro, o con la cuenta o la suscripción |
| Dirección IP al reservar una semana gratis | Nunca se guarda en claro; en memoria, como huella, 15 minutos como máximo |
| Dirección de correo dada para «Obtener un código» | En claro: solo el tiempo de enviar el código; su huella: hasta el final de la beta, como muy tarde el 31 de diciembre de 2026 |
| Huella de la dirección IP (códigos beta, «Obtener un código», página de baja del sitio, denuncias de agentes compartidos) | 2 días como máximo; hasta 30 días en las copias cifradas de la base |
| Baja en el sitio: la declaración (correo, número, motivo, fecha de fin, fecha y hora) | 3 años |
| Baja en el sitio: la dirección de correo escrita, para limitar los intentos | 2 días como máximo |
| Contenido de tu plaza | Durante la suscripción (plaza beta: hasta el final del mes de regalo, el 31 de diciembre de 2026); cuando la suscripción o el mes de regalo termina, la plaza se suspende y se borra 30 días después de su suspensión (3 días después de una semana gratis que no continuó) |
| Copias de seguridad | 2 horas (todas), luego 48 horas (por hora), 21 días (diarias), nada más antiguo; se borran con la plaza; lo que borras ha salido de ellas en 30 días, todas las copias incluidas |
| Archivos de copia que una plaza borra, apartados por el guichet | 5 días |
| Versiones sustituidas o borradas en Scaleway (copias de seguridad de las plazas, copias cifradas de la base de datos del guichet) | 1 día |
| Imagen diaria de OVHcloud | Un punto por día, sustituido en un día aproximadamente después de reinstalar la plaza |
| Datos técnicos de los servidores | Los últimos valores; memoria de cada plaza, hora a hora: 7 días |
| Mediciones de los primeros días de una plaza (sección 5.12) | Por plaza, 11 días como máximo, luego hasta 30 días en las copias cifradas de la base; totales por día, sin nadie: 2 años |
| Registro de eventos del guichet (incluido «código beta usado», con tu cuenta) | 12 meses |
| Metadatos en EUrouter | Hasta 12 meses, según EUrouter |
| Correos de tus agentes (oferta todo incluido) | 90 días por correo (puede acortarse); el buzón entero de una vez cuando quitas la dirección o el agente, cuando se borra la plaza o cuando se borra la cuenta |
| Registros de seguridad del servidor de correo de los agentes | 30 días |
| Denuncias de abusos | 1 año después de su tramitación |

## 13. Seguridad

El relé solo acepta conexiones cifradas (HTTPS), sella los tokens (XChaCha20-Poly1305), no tiene base de datos y
nunca escribe un token en claro. En la app, las claves de tus agentes están protegidas por el Llavero o el Keystore,
y la conexión con tu servidor pasa por Tailscale o por una puerta HTTPS en tu red local, que solo acepta el
certificado de tu servidor. El código de la app, del servidor y del relé es público: https://github.com/Alexbob0/BipAgents. Para
informar de un fallo de seguridad: **security@bipagents.com** (consulta el archivo SECURITY.md del repositorio).

Con la oferta todo incluido, cada plaza es una cuenta separada del servidor, sin derechos de administrador; Kerfy
Studio no tiene ninguna clave SSH en los servidores, y un servidor solo ejecuta órdenes firmadas por el guichet. La
app se conecta a tu plaza en HTTPS y comprueba su certificado. Las copias de seguridad se cifran en la plaza antes de
salir. El servidor de correo de los agentes es un servidor aparte, gestionado por Kerfy Studio: las plazas se conectan
a él con conexiones cifradas, y un agente solo envía correos a través de un control en su plaza que aplica las
direcciones autorizadas, tus aprobaciones y el límite diario.

**En caso de violación de datos**, Kerfy Studio avisa a la CNIL en un plazo de 72 horas (artículo 33 del RGPD) y a las
personas afectadas cuando el riesgo para ellas es alto (artículo 34); para las personas en Singapur, a la Personal
Data Protection Commission en un plazo de 3 días cuando la ley de Singapur lo exige.

## 14. Tus derechos

Puedes solicitar el acceso a tus datos, su rectificación, su supresión, la limitación de su tratamiento y su
portabilidad, y dar instrucciones sobre el destino de tus datos después de tu fallecimiento (derecho francés).

**Derecho de oposición.** Puedes oponerte en cualquier momento, por motivos relacionados con tu situación particular,
a los tratamientos basados en nuestro interés legítimo (limitación de abusos, correos, denuncias, informes de
fallos, límite de las semanas gratis, medición de nuestras campañas, mediciones de los primeros días de una plaza).

**Retirar tu consentimiento.** Para Salud, Calendario, Contactos, Recordatorios, la ubicación de los recordatorios por
lugar y el escáner de documentos de Android, retiras tu consentimiento cuando quieras, tan fácilmente como lo diste:
desactiva la herramienta en la página del agente, o retira el permiso en los ajustes del teléfono (o en Apple Salud o
Health Connect). Ya no se envía nada, y las copias se borran como dice la sección 3. Lo hecho antes sigue siendo
lícito (artículo 7.3 del RGPD).

Escribe a **privacy@bipagents.com**. Respondemos en el plazo de un mes, prorrogable dos meses para una solicitud
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
privacy@bipagents.com.

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
debe usarse en tu propio ordenador o en tu propio servidor, en una instalación preparada y supervisada por uno de los
padres o un adulto a cargo del menor** (función familia): ese adulto elige los agentes, sus accesos y el modelo,
acepta las condiciones de uso por el menor y sigue siendo responsable de su uso. La oferta todo incluido (una plaza
alojada por Kerfy Studio) no se ofrece a personas menores de 16 años, ni siquiera con supervisión. Una persona
invitada en la oferta debe tener al menos 16 años.

Kerfy Studio no pregunta la edad. Algunas funciones se basan en tu consentimiento: Salud, Calendario, Contactos,
Recordatorios, la ubicación de los recordatorios por lugar (sección 3) y el escáner de documentos de Android, por lo
que Google recibe de él (sección 7). Antes de los 15 años, en una instalación en tu propio ordenador o tu propio
servidor, el adulto que la preparó lo da junto con el menor (artículo 45 de la ley francesa de protección de datos). Si un menor nos ha escrito, uno de sus padres puede
pedirnos que borremos su mensaje.

**Oferta todo incluido**: la contrata un adulto, o se contrata con el acuerdo de uno de los padres; toda persona que
usa una plaza, personas invitadas incluidas, tiene al menos 16 años ([condiciones de la oferta](offer.es.md), sección
7). **En Estados Unidos, ninguna persona menor de 13 años puede usar una plaza**, y Kerfy Studio no recoge a sabiendas
ningún dato de un menor de 13 años (Children's Online Privacy Protection Act); si ha ocurrido, escríbenos y lo
borramos. **En Quebec**, los datos de un menor de 14 años solo se recogen con el consentimiento de uno de sus padres o
del tutor: el adulto que prepara y supervisa su instalación, en su propio ordenador o su propio servidor, lo da por él
(artículos 4.1 y 14 de la ley de Quebec).

## 17. Cambios

Podemos modificar esta política, en particular antes de ofrecer enlaces de socios. La fecha al principio de la página
cambia cada vez. Una función nueva que eliges usar se aplica en cuanto la activas: la app te dice entonces qué hace
con tus datos. Un cambio importante que afecta a lo que ya usas se anuncia en la app al menos 30 días antes de
aplicarse.

## 18. Contacto

Kerfy Studio — [dirección del domicilio social] — **privacy@bipagents.com** — abusos: **abuse@bipagents.com**

Esta política, las condiciones de uso, las [condiciones de la oferta todo incluido](offer.es.md), la
[política de uso aceptable](acceptable-use.es.md) y el aviso legal (mentions légales, en francés) también están en la
app: Ajustes › Acerca de.
