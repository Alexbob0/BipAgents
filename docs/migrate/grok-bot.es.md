[🇬🇧 English](grok-bot.en.md) · [🇫🇷 Français](grok-bot.fr.md) · [🇩🇪 Deutsch](grok-bot.de.md) · 🇪🇸 Español

# Venir de Grok Bot (o de ChatGPT dots, Muse, otro agente)

Grok Bot no tiene botón de exportar: la única manera de llevarte lo que sabe es pedirle que lo escriba todo. El
mensaje de abajo le hace escribir su papel, su memoria, las personas, las tareas, las rutinas y las habilidades en
archivos, y luego meterlos en un zip que descargas.

1. Copia el mensaje y pégalo en la conversación con tu Bot (un Bot cada vez).
2. Descarga el archivo `bipagents-export.zip` que adjunta a su respuesta. Revísalo si quieres: puedes quitar lo que
   no quieras llevarte.
3. En BipAgents: **Añadir un agente › Venir de otro asistente › Importar el archivo**. La app te enseña todo lo que
   va a crear antes de guardar nada.

Lo que escribe el Bot es su propia versión de su memoria: puede estar incompleta. Las habilidades llegan apagadas y
las rutinas no se vuelven a programar sin tu sí. No se guarda nada que parezca una contraseña, una clave o un número
de tarjeta.

## El mensaje

```text
Cambio de asistente y quiero llevármelo todo. Crea una carpeta bipagents-export en tu espacio de trabajo y escribe en ella, lo más completos posible, estos archivos Markdown:

1. SOUL.md: tu papel, tu personalidad, tu tono y todas las reglas permanentes que te di, en segunda persona («Eres…», «Nunca haces…»).
2. USER.md: todo lo que sabes de mí: quién soy, mi situación, mis gustos, mis preferencias, cómo escribo.
3. MEMORY.md: todo lo demás de tu memoria, un hecho por línea que empiece por «- », con la fecha (AAAA-MM-DD) cuando la sepas.
4. people.md: cada persona que conoces, una sección «## Nombre» con su relación conmigo y lo que hay que saber de ella.
5. tasks.md: las tareas y proyectos en curso, una línea «- » cada uno, con el plazo y cómo va.
6. routines.md: cada rutina o tarea programada, una sección «## Nombre» con el horario exacto y la zona horaria, la instrucción completa, lo que produce y las apps que usa.
7. skills/<nombre>/SKILL.md: cada habilidad o procedimiento que aprendiste, completo. Empieza por:
---
name: <nombre-en-minusculas-con-guiones>
description: <cuándo usarla, en una frase>
---
y luego cuándo usarla, qué hace falta al principio, los pasos numerados, cómo comprobar y lo que entregas.
8. apps.md: los nombres de las apps y servicios a los que estás conectado, uno por línea, sin credenciales.
9. conversations.md: un resumen por tema de nuestras conversaciones pasadas, una sección «## Tema (periodo)» cada uno, con las decisiones tomadas.

Reglas:
- Sé exhaustivo: mejor de más que de menos. Haz varias pasadas si hace falta. Un archivo sin contenido: escribe «(nada)».
- No inventes nada. Añade «(dudoso)» a lo que no tengas claro.
- NUNCA escribas una contraseña, código, token, clave de API, número de tarjeta o de cuenta bancaria: pon «[quitado]» en su lugar.
- Escribe en el idioma en que te hablo.

Después, haz un solo zip de esa carpeta llamado bipagents-export.zip, adjúntalo a tu respuesta para que lo descargue (si no puedes hacer un zip, adjunta los archivos uno a uno) y responde «Listo» con el número de archivos.
```
