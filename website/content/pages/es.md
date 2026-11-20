<!-- docs -->
# Primeros pasos y compatibilidad

Instala el paquete oficial adecuado y usa tu suscripción o archivo de configuración de Mihomo. Conoce las funciones, los límites y el historial.

## Empezar {#start}

Añade y selecciona una suscripción o importa un YAML en Perfiles. Activa el proxy del sistema y comprueba una conexión. Consulta la [guía de uso](site:guide).

## Funciones actuales {#current-capabilities}

Gestiona suscripciones y nodos y consulta conexiones, registros y diagnósticos. El proxy de IA y la selección manual se excluyen. El [enrutamiento por dominio](site:routing) está desactivado por defecto. No se habilitan nuevas integraciones con Codex ni el proxy por aplicación.

## TUN y límites por plataforma {#tun-compatibility}

En macOS, usa un instalador completo firmado con Developer ID y notarizado por Apple, y autoriza el asistente. TUN en Windows es experimental y requiere permisos de administrador. No se implementa en Linux; el proxy del sistema usa gsettings de GNOME.

## Descarga y arquitectura {#download-center}

En la [página de descarga](site:home#download), elige x64 o ARM64 según tu equipo. Para Mac con serie M, elige Chip de Apple. Las versiones estables se confirman en el [centro de descargas](https://downloads.cmmuu.com/projects/serylane).

## Historial {#history}

Las versiones anteriores están en los archivos [en inglés](/docs/#history) y [en chino simplificado](/zh/docs/#history). No indican cuál es la descarga actual.

<!-- routing -->
# Elegir salidas por dominio

Envía las conexiones coincidentes que ya entran en el núcleo al grupo elegido. El enrutamiento por dominio está desactivado por defecto.

## Herramientas y dominios propios {#node-metadata}

Usa los preajustes OpenAI, Claude y Gemini o tus dominios. `example.com` coincide con el dominio raíz; `*.example.com` solo con subdominios. Desmarcar una herramienta no borra tus dominios.

## Guardar y aplicar {#save-apply}

Elige la salida de la suscripción activa, guarda el borrador y confirma la activación o aplicación. El resto conserva sus reglas. Si no existe la salida, elige otra; no se cambia a conexión directa.

## Controles compartidos {#global-controls}

El botón superior y la página de enrutamiento comparten el estado. Actualizarlo no inicia el proxy ni cambia entre proxy del sistema y TUN.

## Verificar el resultado {#enablement}

Revisa el modo Reglas, las reglas cargadas y la entrada de tráfico. Guardar no confirma la aplicación. Comprueba la regla y salida de una conexión nueva.

## Al detener el proxy {#proxy-routing-state}

Si el núcleo se detiene, el enrutamiento activo queda en pausa sin perder los ajustes. Se comprueba de nuevo al iniciar el proxy expresamente.

## Nodos y estabilidad {#stability}

El proxy de IA admite 1 a 10 nodos aptos y excluye la selección manual. Un solo nodo no tiene respaldo. Cambiarlo solo afecta a conexiones nuevas.

## Límites {#validation-boundaries}

No descifra TLS, no usa rutas cifradas y no cambia Codex. Las nuevas integraciones con Codex, el proxy por aplicación y la antigua estrategia de estabilidad avanzada están desactivados. [Herramientas de IA](site:guide#ai-tools).

<!-- verification -->
# Verificar descargas y actualizaciones

Comprueba el origen oficial, el nombre exacto, la arquitectura, el SHA-256 y la firma. La firma y los permisos del sistema operativo son independientes.

## Origen {#download-origin}

Usa la [página de descarga](site:home#download) o el [centro de descargas](https://downloads.cmmuu.com/projects/serylane). Los enlaces al código de GitHub no son una fuente de descarga alternativa.

## Comparar SHA-256 {#checksums}

Compara el archivo exacto y su hash con `SHA256SUMS.txt` de la misma versión. Si no coinciden, no instales. Sustituye los nombres de ejemplo por los reales.

- Windows: `Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS: `shasum -a 256 ./installer.dmg`
- Linux: `sha256sum ./installer.AppImage`

## Firma de actualización {#update-signatures}

El cliente verifica la firma del proyecto. En una comprobación manual, usa la firma, el manifiesto y la clave pública de esa versión.

## Firma del sistema {#system-signing}

La firma del proyecto no sustituye a Windows Authenticode, Apple Developer ID ni la notarización. Consulta el registro de la versión.

## TUN en macOS {#macos-tun}

Usa el instalador completo firmado y notarizado. Si el asistente antiguo falla con `EX_CONFIG`, debe retirarse su registro y comprobarse el nuevo. Autorízalo en Ítems de inicio si se solicita y valida el tráfico real.

## Descarga interrumpida {#update-downloads}

Se intenta GitHub directo, el acelerador, GitHub por el proxy Serylane activo y el centro de descargas. Cualquier ruta mantiene versión, arquitectura, tamaño, SHA-256 y firma.

## Error de instalación {#update-installation}

Revisa el paso, el espacio libre y los permisos. Si falla un actualizador antiguo, reemplaza la aplicación con el instalador completo adecuado y conserva la configuración.

## Verificaciones antiguas {#verification-history}

Consulta el [archivo en inglés](/docs/verification#verification-history) o [en chino simplificado](/zh/docs/verification#verification-history).

<!-- privacy -->
# Privacidad y datos tratados

El sitio web, el centro de descargas y el cliente de escritorio son independientes. Consultar la web no activa un proxy ni conecta Codex.

## Qué hace el sitio {#website-data}

Ofrece contenido estático e información pública de versiones. No incluye cuentas, analítica o rastreo publicitario ni pide subir configuraciones o conectarse al proxy local.

## Preferencia local {#theme-preference}

Solo se guarda `serylane-theme` para la apariencia; no se envía al servidor. El idioma se indica en la URL, sin guardarlo en el navegador ni usar cookies de seguimiento.

## Alojamiento {#hosting}

Para servir el sitio y protegerlo se procesan datos básicos como IP, hora, ruta y navegador.

## Descargas y enlaces {#downloads}

La web comprueba metadatos públicos y el archivo adecuado sin reenviar cookies ni cabeceras de autorización. Los servicios externos y de descargas procesan sus propias solicitudes. [Verificación](site:verification).

## Cliente de escritorio {#desktop-client}

Las suscripciones y reglas se gestionan en el cliente. El enrutamiento por dominio está desactivado por defecto y no descifra TLS. Oculta credenciales de las URL de suscripción antes de compartir.

## Informar de problemas {#reporting}

Usa los [problemas de GitHub](https://github.com/CMMUU/serylane/issues) sin publicar contraseñas, tokens, URL de suscripciones o configuraciones completas. Comprueba si hay un canal privado para vulnerabilidades.
