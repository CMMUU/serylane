# Guía de uso de Serylane

Instala Serylane, importa tu suscripción o un YAML local, elige un modo proxy y aprende a configurar el enrutamiento, actualizar y resolver fallos.

## Descargar e instalar
<!-- anchor: install -->

Obtén la versión estable verificada en el [sitio oficial](https://serylane.cmmuu.com/es/#download) o el [centro de descargas](https://downloads.cmmuu.com/projects/serylane). El cliente no incluye nodos, suscripciones ni cuentas de IA.

- Windows: x64 en equipos Intel o AMD de 64 bits; ARM64 para Windows en Arm.
- macOS: chip de Apple para la serie M, Intel para un Mac con Intel. Abre el DMG, arrastra Serylane a Aplicaciones y usa macOS 13 o posterior.
- Linux: elige el AppImage adecuado, dale permiso de ejecución y ábrelo.

## Primeros pasos
<!-- anchor: quick-start -->

1. En Suscripciones, introduce el nombre y la URL de una suscripción Mihomo o Clash Meta, valida y añade. Para un YAML local, ábrelo desde Perfiles, compruébalo y actívalo.
2. Comprueba que el perfil esté seleccionado. Seleccionar una configuración no inicia el proxy.
3. Activa Proxy del sistema en la barra superior, abre un sitio conocido y comprueba en Conexiones si la solicitud entró en el núcleo y siguió la regla esperada.

## Elegir un modo proxy
<!-- anchor: proxy-modes -->

- **Proxy del sistema**: para aplicaciones que leen la configuración del sistema. En Linux usa gsettings de GNOME.
- **TUN**: para aplicaciones que ignoran el proxy del sistema. En macOS usa el instalador completo firmado y notarizado, y autoriza el asistente en Ítems de inicio y extensiones si se solicita. En Windows es experimental y requiere privilegios de administrador. No se implementa en Linux.
- **Puerto local**: para configurar un proxy manualmente usa `127.0.0.1` y el puerto mixto mediante HTTP o SOCKS5, no el puerto de control.

Que el asistente esté listo no demuestra que el tráfico pase por TUN. Comprueba el modo y una conexión real.

## Gestionar suscripciones
<!-- anchor: subscriptions -->

Actualiza una tarjeta o todas las suscripciones. El consumo y la configuración se actualizan por separado; si falla, se conserva la última configuración utilizable. Edita con Más o con clic derecho y actualiza tras cambiar el enlace. Si falla la importación, verifica que sea una URL de suscripción Mihomo o Clash Meta y revisa la ruta de descarga.

## Elegir nodos o usar el proxy de IA
<!-- anchor: nodes -->

Los modos se excluyen mutuamente. Puedes elegir un nodo de un grupo y medir su latencia, o usar el proxy de IA con 1–10 nodos válidos. Con uno solo no hay respaldo. Aplica el nodo y espera la confirmación, o vuelve a la selección automática. La selección afecta a nuevas conexiones.

## Herramientas de IA, dominios y Codex
<!-- anchor: ai-tools -->

El enrutamiento por dominio está desactivado por defecto. Solo afecta a solicitudes que ya llegan al núcleo en modo Reglas. Elige herramientas o dominios y la salida, guarda el borrador y luego activa o aplica. `example.com` coincide con el dominio raíz y `*.example.com` solo con subdominios. [Enrutamiento por dominio](https://serylane.cmmuu.com/es/docs/routing).

En macOS y Windows, una CLI de Codex compatible puede iniciarse con `codex --enable respect_system_proxy` para usar la configuración existente del proxy del sistema en esa ejecución. No activa PAC ni añade conmutación por error. Consulta la [documentación de OpenAI](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests) y comprueba que termine una solicitud real.

## Comprobar e instalar actualizaciones
<!-- anchor: updates -->

En Configuración → Actualización, revisa la versión y el origen efectivo. Se intenta GitHub directo, el acelerador GitHub del proyecto, GitHub a través del proxy Serylane activo y el centro de descargas. Se comprueban versión, arquitectura y firma. Si falla el actualizador antiguo, reemplaza la aplicación por el instalador completo correspondiente y conserva la configuración. [Verificación](https://serylane.cmmuu.com/es/docs/verification).

## Resolver incidencias
<!-- anchor: troubleshooting -->

- Una aplicación sigue fuera del proxy: comprueba que el proxy del sistema o TUN esté activo y revisa Conexiones.
- TUN en macOS espera aprobación: comprueba los ítems de inicio. Detén el proxy antes de volver a asociar el asistente.
- Puerto ocupado: revisa el detalle técnico y espera a que TUN libere el puerto antes de reintentar.
- HTTP 401 solo indica que se alcanzó el extremo; valida por separado el inicio de sesión y una respuesta real.

Al informar indica versión, sistema, arquitectura, modo, hora y error depurado. No compartas enlaces de suscripción ni claves.
