# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · **Español**

Una aplicación de escritorio que ejecuta varios proxies y túneles a la vez. En un lienzo se conectan bloques de
**entrada** (cómo entra el tráfico, p. ej. un puerto SOCKS5 local) con bloques de **salida** (cómo sale,
p. ej. un servidor Shadowsocks), y cada tarjeta se enciende o se apaga.

<sub>Hecho para uso personal y compartido tal cual.</sub>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="Lienzo">
  </picture>
</p>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/proxies-dark.png">
    <img src="images/proxies-light.png" width="49%" alt="Proxies">
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/router-dark.png">
    <img src="images/router-light.png" width="49%" alt="Router">
  </picture>
</p>

## Funciones

- **Unir bloques en un lienzo**: entradas (SOCKS5, HTTP, servidor Shadowsocks / Hysteria2 / WireGuard, TUN, ...)
  con salidas (Directo, proxy superior, Shadowsocks, Hysteria2, WireGuard, VLESS, VMess, Trojan, TUIC, suscripciones)
- **Enrutamiento**: repartir el tráfico por dominio, IP, programa o país; un interruptor IF sirve de kill switch; los
  grupos usan el servidor más rápido o cambian si uno falla
- **Cadenas**: enviar una salida a través de otra, o a través de Tailscale, una app de VPN o el Wi-Fi
- **Proxy del sistema**: usar una tarjeta como proxy del sistema de Windows mientras funciona
- **Herramientas de uso diario**: enlaces para compartir, pruebas de conexión, registros y estadísticas en vivo,
  8 idiomas, bandeja

Los proxies funcionan con [sing-box](https://github.com/SagerNet/sing-box), incluido en la versión publicada.

## Ejecutar

Descárgala de [Releases](https://github.com/NenkaLab/proxy-dock/releases):

- **Windows 10 / 11**: `ProxyDock-<versión>-windows-x64-setup.exe` la instala para tu usuario (sin permisos de
  administrador); el `.zip` es la misma aplicación como carpeta.
- **macOS** (Apple Silicon): abre el `.dmg` y arrastra ProxyDock a Aplicaciones. No está firmada por Apple: la primera
  vez, clic derecho → Abrir.
- **Linux** (x64): el `.AppImage` se ejecuta tal cual (antes `chmod +x`); el `.tar.gz` es la misma aplicación como carpeta.

Los ajustes se guardan en `%APPDATA%\ProxyDock` (Windows), `~/Library/Application Support/ProxyDock` (macOS) o
`~/.config/ProxyDock` (Linux); la versión 3 sigue usando los ajustes de la 2.x. La página **Cómo usar** de la aplicación
explica el resto. ProxyDock 3 es una reescritura en Rust; la 2.x (la aplicación Python) está en la rama [`old`](https://github.com/NenkaLab/proxy-dock/tree/old).

## Compilar

Ejecutar desde el código fuente, pruebas, compilación, instaladores y versiones: [Build.md](Build.md) (en inglés).
Comprobaciones antes de publicar: [TESTING.md](../TESTING.md) (en coreano).

## Limitaciones

- macOS y Linux se compilan y prueban automáticamente, pero aún no se han probado en equipos reales.
- macOS / Linux: no hay regla que bloquee otros PC (usa el cortafuegos del sistema), no hay «Ejecutar como
  administrador» al iniciar (una tarjeta TUN pide la contraseña al iniciarse), no se lee el perfil de color del monitor
  (se supone sRGB), las contraseñas de los ajustes no se cifran (como en la 2.x) y los botones atrás / adelante del
  ratón no navegan.
- Linux: el proxy del sistema (bloque PC) solo se configura en GNOME y KDE; las notificaciones necesitan `notify-send`.
- TUN todavía no se ha probado en equipos reales.
- SOCKS4 no tiene contraseña: úsalo solo en este PC.

## Licencia

[GPL-3.0](../LICENSE). Los componentes incluidos (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, crates de Rust, fuentes OFL-1.1, ...)
y sus licencias están en [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
