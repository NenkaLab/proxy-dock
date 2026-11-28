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
- **Línea directa**: los bloques elegidos salen directamente por una tarjeta de red, saltándose una VPN, un TUN o
  cualquier app que tome la red de todo el PC, sin su túnel dividido (Windows: una pequeña VM de Hyper-V; Linux: un
  espacio de nombres de red; macOS: una pequeña VM, aún sin probar)
- **Inicio**: un interruptor para toda la app (apagado rechaza toda conexión, un kill switch; cada tarjeta mantiene
  su propio encendido / apagado), el estado de las tarjetas, el tráfico en vivo y las VM de línea directa

Los proxies funcionan con [sing-box](https://github.com/SagerNet/sing-box), incluido en la versión publicada.

## Ejecutar

Descárgala de [Releases](https://github.com/NenkaLab/proxy-dock/releases):

- **Windows 10 / 11**: `ProxyDock-<versión>-windows-x64-setup.exe` la instala para tu usuario (sin permisos de
  administrador); el `.zip` es la misma aplicación como carpeta.
- **macOS** (Apple Silicon): abre el `.dmg` y arrastra ProxyDock a Aplicaciones. No está notarizada por Apple, así que
  macOS bloquea el primer inicio: Ajustes del Sistema → Privacidad y seguridad → **Abrir igualmente** (clic derecho →
  Abrir ya no sirve desde macOS 15), o quita la marca de descarga en Terminal:
  ```
  xattr -dr com.apple.quarantine /Applications/ProxyDock.app
  ```
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
- Línea directa: en Windows necesita Hyper-V (Pro, Enterprise o Education) y permisos de administrador; cada VM
  aparece en tu router como un dispositivo nuevo (sus propias direcciones MAC e IP), y un proveedor que da una sola
  dirección por línea puede no darle ninguna. Su tráfico muestra tu IP real y ninguna VPN lo protege: eres
  responsable de usarla (la app pide tu consentimiento antes de crear una VM).
- Línea directa en Linux: probada solo en un contenedor (un espacio de nombres de red con macvlan; el Wi-Fi usa
  ipvlan, donde DHCP puede no funcionar: usa una dirección fija). Necesita `ip` (iproute2) y root mediante pkexec.
- Línea directa en macOS (Apple Silicon): escrita y comprobada en tipos, pero **aún sin probar en ningún Mac**
  (Virtualization.framework con una interfaz vmnet en puente; el asistente debe ejecutarse como root).

## Contribuir

Si corregiste o mejoraste algo – un error, una plataforma aún sin probar (macOS, Linux), una traducción –, envía un
pull request. Hecho a mano o con una herramienta de IA, da igual, pero compruébalo antes de enviarlo: compílalo,
ejecuta las pruebas (`cargo test --workspace` en `native/`) y prueba el cambio en la app, en una máquina real cuando
importe. Indica en el pull request qué comprobaste y cómo.

## Licencia

[GPL-3.0](../LICENSE). Los componentes incluidos (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, crates de Rust, fuentes OFL-1.1, ...)
y sus licencias están en [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
