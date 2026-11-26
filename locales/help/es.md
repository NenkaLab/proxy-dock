## Lo básico

- Los bloques de **entrada** aceptan conexiones en este PC, por ejemplo un puerto SOCKS5.
- Los bloques de **salida** son por donde sale el tráfico: Directo, otro proxy o un túnel.
- En el **Lienzo**, arrastra la salida de un bloque a la entrada de otro para unirlos. Haz doble clic en un bloque para editarlo.
- Inicia una entrada con **Ejecutar** en su bloque o desde el menú de la bandeja.

## Usarlo en una app

- Clic derecho en una entrada → **Copiar dirección/enlace para clientes** y ponla como proxy de la app.
- Todo el PC: clic derecho en una entrada HTTP → **Usar como proxy del PC** (proxy del sistema de Windows mientras funciona).
- Móviles y otros PC: copia el enlace de una entrada de servidor Shadowsocks, Hysteria2 o WireGuard.

## Bloques intermedios

- **Router**: reglas por dominio, IP, programa o país, cada una con su propia salida.
- **IF**: una condición (entonces / si no), o un interruptor manual como un kill switch.
- **Grupo**: varias salidas como una: la más rápida, la primera que funcione o la que elijas.
- **via**: la conexión de una salida pasa por otra salida (cadena).
- **Paso directo** y **par inalámbrico**: solo ordenan las líneas.

## Teclas del lienzo

| Tecla | Acción |
|---|---|
| Ctrl+Z / Ctrl+Y | Deshacer / rehacer |
| F2 · Ctrl+D · Supr | Cambiar nombre · duplicar · eliminar |
| ← → | Bloque anterior / siguiente por las líneas |
| ↑ ↓ | Bloque de arriba / abajo (tras ← →, la otra opción si hay varias) |

## Otros PC

Pon la dirección de la entrada en `0.0.0.0` y activa una contraseña. En Windows, la regla del firewall está en **Configuración**.

## Cerrar

X solo oculta la ventana; los proxies siguen en la bandeja. Sal desde el menú de la bandeja.
