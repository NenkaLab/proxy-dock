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

## VPN bypass (Windows)

With a VPN on for the whole PC, a block can still leave outside it: set its **Network interface** to **Outside the VPN**. A small Hyper-V virtual machine connected straight to a network card carries that traffic (**Settings > VPN bypass**: network card, memory, address; **Create** asks for your consent first). Needs Windows Pro, Enterprise or Education with Hyper-V on.

That traffic shows your real IP address and the VPN's kill switch and DNS protection do not cover it; with a company or school VPN this may break their rules. You are responsible for what follows from turning it on.

## Other PCs

Set the inbound address to `0.0.0.0` and turn on a password. On Windows, the firewall rule is under **Settings**.

## Closing

X hides the window and the proxies keep running in the tray. Quit from the tray menu.
