# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · **Español**

Una aplicación de escritorio que ejecuta varios proxies y túneles a la vez. En un lienzo se conectan bloques de
**entrada** (cómo entra el tráfico, p. ej. un puerto SOCKS5 local) con bloques de **salida** (cómo sale,
p. ej. un servidor Shadowsocks), y cada tarjeta se enciende o se apaga.

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

- **Entrada**: SOCKS5, SOCKS4, HTTP, HTTPS, servidor Shadowsocks, servidor Hysteria2, servidor WireGuard (teléfonos y PC se conectan con la aplicación WireGuard), reenvío de puertos, TUN (todas las aplicaciones del PC)
- **Salida**: Direct, proxy HTTP / HTTPS / SOCKS, Shadowsocks, Hysteria2, WireGuard, VLESS (REALITY), VMess,
  Trojan, TUIC, suscripciones (enlaces compartidos o YAML de Clash)
- **Bloques**: cadenas (una salida a través de otra), routers (reglas por dominio, IP, puerto, programa - elegible entre los que están en ejecución -, GeoIP/Geosite; vistas sencilla y avanzada),
  IF (una condición, o un interruptor manual como un kill switch), grupos (el más rápido, conmutación por error o manual), Bloquear
- **Interfaz de red**: enviar una salida por Tailscale, una aplicación VPN o Wi-Fi sin cambiar las rutas del PC
- **DNS**: el DNS del PC, DoH / DoT / DoQ, por tarjeta, opcionalmente por el túnel; Fake-IP para TUN
- **Tarjetas**: ejecutar, iniciar con la aplicación y reiniciar automáticamente (si se detiene o no arranca) desde el propio bloque
- **Bloque PC**: usar una entrada HTTP como proxy del sistema de Windows mientras funciona
- Enlaces compartidos (`ss://`, `vless://`, `vmess://`, `trojan://`, `tuic://`, `hysteria2://`), comprobación y prueba de conexión,
  registros y estadísticas en vivo, 8 idiomas, bandeja, inicio al iniciar sesión

Los proxies funcionan con [sing-box](https://github.com/SagerNet/sing-box) (un proceso por tarjeta, incluido en la versión publicada).
Un servidor Hysteria2 abierto a otros PC usa el núcleo oficial de [Hysteria](https://github.com/apernet/hysteria);
el motor Python integrado queda como respaldo.

## Ejecutar

Descarga una versión de [Releases](https://github.com/NenkaLab/proxy-dock/releases) o ejecuta desde el código fuente
(Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # sing-box y Hysteria para este PC
python proxydock.py
```

Los ajustes se guardan en `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`,
Linux: `~/.config/ProxyDock`). La página **Cómo usar** de la aplicación explica el resto.

## Compilar

`tools\build_win.bat` → `dist/win/ProxyDock.exe` (Linux: `tools/build_linux.sh`, macOS: `tools/build_mac.sh`).
La compilación descarga sing-box y Hysteria (con SHA-256 comprobado) y los mete en la aplicación.
Pruebas: `python tests/run_tests.py`. Versiones: `tools\release.bat <versión>` crea la etiqueta y hace push; GitHub Actions compila.

## Limitaciones

- Routers, grupos, WireGuard, VLESS / VMess / Trojan / TUIC, TUN, elección de interfaz y DNS propio requieren el motor sing-box.
- No hay balanceo de carga entre servidores; un servidor Hysteria2 no puede escuchar por sí solo en un rango de puertos.
- La fijación de certificados funciona con el motor Python hasta sing-box 1.15.
- TUN y Fake-IP aún no se han probado en entornos reales, ni macOS en un Mac real.
- SOCKS4 no tiene contraseña: úsalo solo en este PC.

## Licencia

[GPL-3.0](../LICENSE). Los componentes incluidos (sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, fuentes OFL-1.1, ...)
y sus licencias están en [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
