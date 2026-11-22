# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · **Español**

Un "dock" de proxies para el escritorio: crea varios proxies y activa o desactiva cada uno. Hecho para uso personal.

Cada proxy combina una **entrada** (cómo acepta conexiones en un puerto de este PC) y una **salida**
(cómo llega al destino). Por ejemplo, "recibir en un SOCKS5 local → túnel cifrado hacia un servidor Shadowsocks remoto"
es una sola tarjeta.

<p align="center">
  <img src="images/proxies.png" width="49%" alt="Lista de proxies">
  <img src="images/editor.png" width="49%" alt="Editor de proxy">
</p>

## Funciones

| | Compatible |
|---|---|
| Entrada | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · servidor Shadowsocks · servidor Hysteria2 |
| Salida | Direct · proxy superior HTTP / HTTPS / SOCKS4 / SOCKS5 · túnel Shadowsocks · túnel Hysteria2 (QUIC) |
| UDP | entrada SOCKS5 · Shadowsocks · Hysteria2, salida Direct · Shadowsocks · Hysteria2 |

- **Shadowsocks**: AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) y 2022 (SIP022, blake3), TCP+UDP, copia de enlaces `ss://`
- **Hysteria2**: compatible en ambos sentidos con la implementación oficial (TCP y UDP), fijación del certificado, copia de enlaces `hysteria2://`
- **Configuración**: valores globales predeterminados + configuración local por proxy (la local manda) - dominios permitidos, bloqueo de red privada, tiempo de inactividad, registro
- **Comprobar conexión** en el editor: prueba la salida (superior/túnel, contraseñas) y el puerto de entrada antes de guardar
- **Prueba de conexión**: se conecta por HTTPS a 21 sitios de muchos países a través de un proxy en ejecución y mide los tiempos de respuesta
- **Registro / estadísticas** por proxy, gráfico de rendimiento en vivo, archivo de registro opcional
- **Seguridad**: contraseñas guardadas con Windows DPAPI (archivos 0600 en otros sistemas), límite de conexiones por IP,
  bloqueo temporal tras fallos de autenticación repetidos, detección de bucles en cadenas de proxies, botón de bloqueo del firewall de Windows
- **Idiomas**: English (predeterminado), 한국어, 日本語, 简体中文, 繁體中文, Deutsch, Français, Español (Configuración → Pantalla → Idioma)
- Bandeja del sistema, inicio al iniciar sesión, tema claro/oscuro, fuentes integradas (Pretendard / JetBrains Mono)

## Ejecución

Usa una compilación de [Releases](https://github.com/NenkaLab/proxy-dock/releases) si la hay, o ejecútalo desde el código fuente:

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS, Linux: source .venv/bin/activate
pip install -r requirements.txt
python proxydock.py            # --hidden : iniciar en la bandeja sin ventana
```

Desarrollado y probado con Python 3.14.

En el primer inicio no hay proxies. Elige una plantilla en **Nuevo proxy** (SOCKS5 local, HTTP local, servidor Shadowsocks,
Local → túnel Shadowsocks, ...) e iníciala con el interruptor de su tarjeta. La página **Cómo se usa** de la aplicación tiene
los detalles. La interfaz está en inglés de forma predeterminada; cambia el idioma en Configuración → Pantalla → Idioma
(se aplica tras reiniciar).

Ubicación de la configuración

| Sistema | Ruta |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (predeterminado `~/.config/ProxyDock`) |

## Compilación

Todo lo necesario para compilar está en [`tools/`](../tools/). PyInstaller genera un único ejecutable (una `.app` en macOS).
Los scripts crean un entorno virtual por sistema, instalan solo los paquetes que difieren de `requirements.txt` y compilan con
`tools/ProxyDock.spec`. Se pueden ejecutar desde cualquier carpeta.

| Comando | Resultado |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (en Windows) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (de forma nativa en Linux, con Docker en Windows/macOS) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (solo en macOS) |
| `tools\build.bat` / `tools/build.sh` | todo lo que este equipo puede compilar, con un resumen |

La compilación para macOS aún no se ha probado en un Mac real.

## Limitaciones conocidas

- Hysteria2 funciona sobre una implementación de QUIC en Python (aioquic), con unos 100 Mbps. No se admiten ajustes de ancho de banda (Brutal),
  la ofuscación Salamander ni el salto de puertos.
- El servidor oficial de Hysteria2 rechaza las conexiones cuyo SNI no coincide con su certificado: escribe el dominio del
  certificado del servidor en el campo SNI de la salida.
- La entrada HTTP atiende una sola solicitud normal (no CONNECT) por conexión.
- SOCKS4 no tiene autenticación por contraseña, así que úsalo solo en este PC.
- El japonés y el chino usan la fuente de interfaz del sistema (el Pretendard integrado cubre el coreano y el alfabeto latino).

## Estructura

```
proxydock.py            punto de entrada
proxydock/
  common.py             constantes comunes, registro, manejo de direcciones, bloqueos
  config.py             formato de configuración (v2), validación, fusión global/local, almacenamiento de contraseñas, migración
  runtime.py            ProxyInstance / ProxyManager (varios proxies en un solo hilo asyncio)
  relay.py              núcleo del relé TCP/UDP
  inbound.py            protocolos de entrada
  outbound.py           protocolos de salida
  ss.py                 cifrado Shadowsocks 2018/2022
  hysteria2.py          servidor y cliente Hysteria2 (aioquic)
  tls.py                certificados autofirmados, contextos TLS
  probe.py              prueba de conexión / comprobación de conexión
  platform_utils.py     inicio al iniciar sesión, firewall, comprobación de puertos
  i18n.py, locales/     traducciones (textos fuente en inglés -> <idioma>.json)
  ui/                   interfaz con PySide6 + Fluent Widgets
tools/                  scripts de compilación, spec de PyInstaller, Dockerfile de compilación para Linux
licenses/               licencias de los componentes de terceros incluidos
docs/                   README traducidos, capturas de pantalla
```

### Traducciones

Los textos de la interfaz están escritos en inglés en el código (`_t("...")`) y se traducen con `proxydock/locales/<idioma>.json`.
Las entradas que faltan se muestran en inglés.

## Licencia

[GPL-3.0](../LICENSE). La biblioteca de interfaz [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) es
GPL v3, por lo que este programa también se distribuye bajo GPL v3; distribuir ejecutables compilados exige cumplir las
condiciones de la GPL v3 (proporcionar el código fuente, etc.).

Las licencias de las fuentes y bibliotecas incluidas están en la carpeta [`licenses/`](../licenses/).

| Componente | Licencia |
|---|---|
| Pretendard, JetBrains Mono (fuentes integradas) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs | MIT |
| certifi | MPL 2.0 |
| Entorno de ejecución de Python | PSF License |
| Cargador de arranque de PyInstaller | GPL v2+ (excepción del cargador de arranque) |

Copyright (C) 2026 NenkaLab
