## Basics

- **Inbound** blocks accept connections on this PC, for example a SOCKS5 port.
- **Outbound** blocks are where the traffic leaves: Direct, another proxy or a tunnel.
- On the **Canvas**, drag from a block's output to another block's input to link them. Double-click a block to edit it.
- Start an inbound with **Run** on its block, or from the tray menu.

## Use it in an app

- Right-click an inbound → **Copy address/link for clients**, and enter it as the app's proxy.
- Whole PC: right-click an HTTP inbound → **Use as the PC proxy** (Windows system proxy while it runs).
- Phones and other PCs: copy the link of a Shadowsocks, Hysteria2 or WireGuard server inbound.

## Blocks between

- **Router**: rules by domain, IP, program or country, each with its own output.
- **IF**: one condition (then / else), or a manual on/off switch such as a kill switch.
- **Group**: several outbounds as one: the fastest, the first that works, or one you pick.
- **via**: an outbound's own connection goes through another outbound (a chain).
- **Pass-through** and **wireless pair**: only tidy up the wiring.

## Canvas keys

| Key | Action |
|---|---|
| Ctrl+Z / Ctrl+Y | Undo / redo |
| F2 · Ctrl+D · Delete | Rename · duplicate · delete |
| ← → | The block before / after along the links |
| ↑ ↓ | The block above / below (or another choice after ← →) |

## Other PCs

Set the inbound address to `0.0.0.0` and turn on a password. On Windows, the firewall rule is under **Settings**.

## Closing

X hides the window and the proxies keep running in the tray. Quit from the tray menu.
