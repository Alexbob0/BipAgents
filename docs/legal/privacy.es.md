[English](privacy.en.md) · [Français](privacy.fr.md) · [Deutsch](privacy.de.md) · Español

# Política de privacidad de BipAgents

> Última actualización: [fecha].

## En resumen

- BipAgents conecta tu teléfono con **tus propios agentes de IA**, que funcionan en tu ordenador o tu servidor con
  Hermes Agent. Tus conversaciones, tus archivos, tu voz y la memoria de tus agentes se quedan en tu servidor y en el
  proveedor de modelos de IA que **tú** has elegido. **Kerfy Studio, el editor de la app, no los ve.**
- Sin cuenta, sin publicidad, sin estadísticas de uso, sin rastreo.
- Kerfy Studio solo puede recibir tres cosas:
  1. lo necesario para entregar tus **notificaciones** a través de su relé: un token de dispositivo sellado, nunca el
     texto de tus mensajes;
  2. lo que decidas enviarnos: un **correo** al soporte o la **denuncia** de una respuesta desde la app;
  3. **informes de fallos**, solo si has activado su envío en los ajustes de tu teléfono.

## 1. Quién es responsable

**Kerfy Studio**, SAS (sociedad por acciones simplificada francesa) con un capital de [capital] €, con domicilio
social en [dirección del domicilio social], inscrita en el RCS de [ciudad del registro] con el número [SIREN]
(«Kerfy Studio», «nosotros»). Contacto: **support@kerfystudio.com**.

Kerfy Studio no ha designado un delegado de protección de datos: no es obligatorio para su actividad (artículo 37 del
RGPD). Para cualquier pregunta sobre tus datos: support@kerfystudio.com.

Kerfy Studio es responsable de los tratamientos que la app realiza para funcionar, tal como Kerfy Studio los ha
diseñado (secciones 3, 4 y 6), incluso cuando se quedan en tu teléfono y Kerfy Studio no tiene acceso a ellos. No es
ni responsable ni encargado del tratamiento de los datos que tratan tu servidor, tus agentes y tu proveedor de modelos
(sección 2).

## 2. Cómo funciona BipAgents y quién ve qué

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

**Lo que se conserva.** El relé no tiene base de datos. El token sellado se guarda en tu teléfono y en tu servidor, no
en nuestros sistemas. Los registros técnicos del relé contienen la plataforma, una huella de 4 bytes del token y la
respuesta de Apple o de Google; se conservan **6 meses como máximo**. El proveedor de alojamiento del relé puede
registrar las direcciones IP de las solicitudes: [lo que registra el proveedor de alojamiento del relé y durante
cuánto tiempo].

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

## 5. Lo que Kerfy Studio no hace

- Sin cuenta, sin publicidad, sin herramienta de estadísticas, sin rastreo entre apps o sitios.
- Sin venta ni alquiler de datos.
- Sin entrenamiento de modelos de IA con tus datos: no los tenemos.
- Sin acceso a tu servidor, a tus conversaciones ni a tus archivos.

## 6. Los demás servicios que intervienen en la app

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

## 7. La demo

La demo integrada funciona por completo en tu teléfono: agentes y datos inventados, sin cuenta, sin servidor, y no se
envía nada a Kerfy Studio. Puedes abrirla desde la primera apertura, con «Solo la demo». Su pódcast es un archivo de
audio grabado de antemano e incluido en la app (voces: Fish Audio Drama 3, «Built with Fish Audio»). Si hablas con la
demo, se aplica el reconocimiento de voz del teléfono (sección 6).

## 8. Las voces

Las voces de los Bips se sintetizan **en tu servidor**: el texto que se va a leer va a tu servidor, no a nosotros. Si
conectas tu propio modelo de voz, es un servidor que tú mismo haces funcionar. Kerfy Studio no recibe nada.

## 9. Enlaces de socios (alojamiento, más adelante)

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
anunciará como se indica en la sección 16.

## 10. Transferencias fuera de la Unión Europea

El relé de notificaciones está alojado en la Unión Europea ([proveedor de alojamiento del relé]). Para entregar las
notificaciones, Apple y Google pueden tratar el token y la notificación fuera de la Unión Europea, en particular en
Estados Unidos:

- Google LLC (Firebase Cloud Messaging) está certificada en el «EU-U.S. Data Privacy Framework», reconocido como
  garante de una protección adecuada por la Decisión de Ejecución (UE) 2023/1795 de la Comisión, de 10 de julio de
  2023;
- Apple indica que enmarca las transferencias desde el Espacio Económico Europeo con las cláusulas contractuales tipo
  adoptadas por la Comisión Europea.

Proveedor de correo: [país; garantía]. Puedes pedirnos una copia de las garantías aplicables.

## 11. Plazos de conservación

| Datos | Plazo |
|---|---|
| Datos de la app en tu teléfono | Hasta que elimines el agente o la app |
| Token de dispositivo en el relé | Ninguna conservación: solo existe sellado, en tu teléfono y tu servidor |
| Dirección IP en el relé | En memoria, una hora como máximo, en forma de huella |
| Registros técnicos del relé | 6 meses como máximo |
| Correos de soporte | 2 años después del último intercambio |
| Denuncias | 1 año después de su tramitación |
| Informes de fallos | 1 año después de su recepción (en Kerfy Studio) |

## 12. Seguridad

El relé solo acepta conexiones cifradas (HTTPS), sella los tokens (XChaCha20-Poly1305), no tiene base de datos y
nunca escribe un token en claro. En la app, las claves de tus agentes están protegidas por el Llavero o el Keystore,
y la conexión con tu servidor pasa por Tailscale o por una puerta HTTPS en tu red local, que solo acepta el
certificado de tu servidor. El código de la app, del servidor y del relé es público: [URL del repositorio]. Para
informar de un fallo de seguridad: **support@kerfystudio.com** (consulta el archivo SECURITY.md del repositorio).

## 13. Tus derechos

Puedes solicitar el acceso a tus datos, su rectificación, su supresión, la limitación de su tratamiento y su
portabilidad, y dar instrucciones sobre el destino de tus datos después de tu fallecimiento (derecho francés).

**Derecho de oposición.** Puedes oponerte en cualquier momento, por motivos relacionados con tu situación particular,
a los tratamientos basados en nuestro interés legítimo (limitación de abusos, correos, denuncias, informes de
fallos).

Escribe a **support@kerfystudio.com**. Respondemos en el plazo de un mes, prorrogable dos meses para una solicitud
compleja; podemos pedirte lo necesario para verificar tu identidad.

Como no conservamos nada que relacione un token de notificación o una denuncia con una persona, en general no podemos
encontrar esos datos sobre ti: para dejar de recibir notificaciones, desactívalas o desinstala la app. Los datos de
tu servidor los gestionas tú; para los de tu proveedor de modelos o tu proveedor de alojamiento, dirígete a ellos.

## 14. Reclamación

Si consideras que no se respetan tus derechos, puedes presentar una reclamación ante la autoridad francesa de
protección de datos, la **CNIL** (3 place de Fontenoy, TSA 80715, 75334 Paris Cedex 07, Francia —
https://www.cnil.fr/fr/plaintes), o ante la autoridad de protección de datos de tu país.

## 15. Menores

BipAgents está dirigida a personas **de 16 años o más**. Entre los 16 y los 18 años, úsala con el acuerdo de uno de
tus padres y respeta la edad mínima de tu proveedor de modelos (a menudo 18 años). **Antes de los 16 años, la app solo
debe usarse en una instalación preparada y supervisada por uno de los padres o un adulto a cargo del menor** (función
familia): ese adulto elige los agentes, sus accesos y el modelo, acepta las condiciones de uso por el menor y sigue
siendo responsable de su uso.

Kerfy Studio no pregunta la edad y ninguno de sus tratamientos se basa en el consentimiento. Solo el escáner de
documentos de Android pide un acuerdo, por lo que Google recibe de él (sección 6): para un menor de 15 años, el adulto
que preparó la instalación lo da junto con el menor. Si un menor nos ha escrito, uno de sus padres puede pedirnos que
borremos su mensaje.

## 16. Cambios

Podemos modificar esta política, en particular antes de ofrecer enlaces de socios. La fecha al principio de la página
cambia cada vez. Un cambio importante se anuncia en la app al menos 30 días antes de aplicarse.

## 17. Contacto

Kerfy Studio — [dirección del domicilio social] — **support@kerfystudio.com**

Esta política, las condiciones de uso y el aviso legal (mentions légales, en francés) también están en la app:
Ajustes › Acerca de.
