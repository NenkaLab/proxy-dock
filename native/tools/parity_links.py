"""Share links, subscription lists and Clash YAML files, read by the Python app and by the Rust core
(examples/dump_links.rs), compared. Random cases around the real formats, plus fixed ones.

    python native/tools/parity_links.py [count] [--lang ko] [--seed N]
"""
import base64
import contextlib
import io
import json
import os
import random
import subprocess
import sys
import tempfile
import time

ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyapp  # noqa: E402,F401  (the Python app: see pyapp.py)
from proxydock import i18n, links, subscription  # noqa: E402

R = random.Random()

HOSTS = ["example.com", "1.2.3.4", "[2001:db8::1]", "[::1]", "", "[1.2.3.4]", "[zz]", "a.b", "[v1.x]", "h:1:2",
         "[fe80::1%25eth0]", "xn--bcher-kva.ch", "ho st", "a[b", "x]"]
PORTS = ["443", "0", "65535", "65536", "", "8388", "x", "0443", "443,5000-6000", "20000-30000", "1-2,3", "99999999999"]
WORDS = ["", "a", "pw", "p%40ss", "%E4%BD%A0", "%ZZ", "%", "a+b", "uuid-1234", "user:pass", "YWVzLTI1Ni1nY206cGFzcw",
         "YWVzLTI1Ni1nY206cGFzc0BleGFtcGxlLmNvbTo4Mzg4", "x y", "%e4%b8", "한글", "%C3%28"]
KEYS = ["sni", "peer", "allowInsecure", "insecure", "allow_insecure", "alpn", "fp", "type", "path", "host", "serviceName",
        "security", "flow", "pbk", "sid", "mport", "pinSHA256", "obfs", "obfs-password", "congestion_control", "udp_relay_mode"]
VALS = ["", "1", "true", "TRUE", "0", "yes", "h2, http/1.1", "chrome", "ws", "grpc", "tcp", "raw", "kcp", "WS", "/p", "tls",
        "reality", "none", "xtls", "salamander", "gost", "bbr", "quic", "a.com", "%2Fx", "a+b", "AA:BB"]
SCHEMES = ["vless", "vmess", "trojan", "tuic", "ss", "hysteria2", "hy2", "VLESS", "http", "Hy2", ""]


def pick(xs):
    return R.choice(xs)


def query():
    parts = []
    for _ in range(R.randint(0, 6)):
        k = pick(KEYS)
        parts.append(k if R.random() < 0.1 else f"{k}={pick(VALS)}")
    if R.random() < 0.1:
        parts.append("")
    return "&".join(parts)


def b64(text, urlsafe=None, pad=None):
    raw = text.encode()
    e = base64.urlsafe_b64encode(raw) if (urlsafe if urlsafe is not None else R.random() < 0.5) else base64.b64encode(raw)
    e = e.decode()
    if pad is False or (pad is None and R.random() < 0.5):
        e = e.rstrip("=")
    return e


def vmess_json():
    d = {}
    for k in ["add", "port", "id", "aid", "scy", "net", "type", "host", "path", "tls", "sni", "alpn", "fp", "ps", "v"]:
        if R.random() < 0.75:
            d[k] = pick({
                "add": HOSTS + ["  a.com "], "port": PORTS + [443, 443.0, True, None, 0, 70000],
                "aid": ["0", "2", 0, 1, "x", None, ""], "scy": ["", "AUTO", "aes-128-gcm", "none"],
                "net": ["", "tcp", "ws", "WS", "grpc", "kcp", "h2"], "type": ["", "none", "http", "None"],
                "tls": ["", "tls", "TLS", "none"], "ps": ["", "My VM", "  x  ", 5, None],
            }.get(k, WORDS + VALS + [None, 1, False]))
    return json.dumps(d)


def link():
    s = pick(SCHEMES)
    if s.lower() == "vmess":
        body = b64(vmess_json()) if R.random() < 0.9 else pick(["!!", "e30", "W10", b64("[1]"), b64("{bad"), "/w"])
        return f"{s}://{body}" + (f"#{pick(WORDS)}" if R.random() < 0.3 else "")
    user = pick(WORDS)
    host, port = pick(HOSTS), pick(PORTS)
    if R.random() < 0.6:        # mostly well-formed, so the read path is covered too
        host, port = pick(["example.com", "1.2.3.4", "[2001:db8::1]", "a.b"]), pick(["443", "8388", "443,5000-6000"])
        user = pick(["pw", "uuid-1234", "user:pass", "YWVzLTI1Ni1nY206cGFzcw", "p%40ss", "11111111-2222-3333-4444-555555555555"])
    if s.lower() == "ss" and R.random() < 0.3:
        netloc = b64(f"{pick(['aes-256-gcm', 'chacha20-ietf-poly1305', ''])}:{pick(WORDS)}@{host}:{port}")
        if R.random() < 0.2:
            netloc = netloc[:-1]
    else:
        netloc = (f"{user}@" if R.random() < 0.85 else "") + (f"{host}:{port}" if R.random() < 0.9 else host)
    tail = pick(["", "/", "/?", "?", "/path?"]) + (query() if R.random() < 0.8 else "")
    frag = f"#{pick(WORDS + ['My%20Server', '  name  '])}" if R.random() < 0.6 else ""
    text = f"{s}://{netloc}{tail}{frag}"
    if R.random() < 0.05:
        text = "  " + text + "\t"
    if R.random() < 0.03:
        i = R.randrange(len(text))
        text = text[:i] + pick(["\t", "\n", "\r"]) + text[i:]
    return text


SCALARS = ["", "x", "yes", "no", "on", "Off", "~", "null", "1", "010", "0x1F", "08", "1:30", "1.5", "1.", ".5", "1e5",
           "1.0e+3", ".inf", "-.Inf", ".nan", "'443'", '"on"', "true", "1_000", "0b101", "+12", "-0", "2024-01-01",
           "!!str 5", "[a, '']", "{}", "AES-256-GCM"]


def clash_entry():
    t = pick(["ss", "ss", "ss", "vmess", "vless", "trojan", "hysteria2", "tuic", "socks5", "http", "wireguard", "snell", "SS"])
    keys = {"name": SCALARS + ["My Node", "노드"], "type": [t], "server": ["1.2.3.4", "a.com", "'[::1]'", "", "~"],
            "port": ["443", "'443'", "' 443 '", "443.9", "x", "~", "yes", "70000", "[1]"],
            "cipher": ["aes-256-gcm", "AES-128-GCM", "auto", "", "2022-blake3-aes-128-gcm"], "password": SCALARS,
            "uuid": ["11111111-2222-3333-4444-555555555555", "", "x"], "alterId": ["0", "1", "x", "~", "''", "2.5"],
            "tls": SCALARS[:6] + ["true", "false"], "network": ["ws", "grpc", "tcp", "h2", "", "WS"],
            "ws-opts": ["{path: /p, headers: {Host: cdn.com}}", "{headers: {host: h}}", "{}", "x"],
            "grpc-opts": ["{grpc-service-name: svc}", "{}", "3"], "servername": ["a.com", "", "1.2.3.4"], "sni": ["b.com", ""],
            "skip-cert-verify": ["true", "false", "yes", "1"], "alpn": ["[h2, http/1.1]", "[]", "h2", "['', ' ']"],
            "client-fingerprint": ["chrome", "", "safari"], "flow": ["xtls-rprx-vision", ""],
            "reality-opts": ["{public-key: K, short-id: ab}", "{}", "x"],
            "auth": ["pw", ""], "obfs": ["salamander", "", "x"], "obfs-password": ["o", ""],
            "up": ["100 Mbps", "1 Gbps", "100", "1.2.3", "abc", "500 kbps", "0.5 g"],
            "down": ["50", "1 Gbps", "x mbps"], "ports": ["20000-30000", "443/5000-6000", ""], "hop-interval": ["30", "0", "x", "10.5"],
            "congestion-controller": ["BBR", "cubic", ""], "udp-relay-mode": ["Native", "quic"], "username": ["u", ""],
            "private-key": ["KEY=", ""], "public-key": ["PUB=", ""], "pre-shared-key": ["", "PSK="],
            "ip": ["10.0.0.2", "10.0.0.2/32", ""], "ipv6": ["fd00::2", ""], "allowed-ips": ["[0.0.0.0/0, '::/0']", "[]", "x"],
            "mtu": ["1280", "0", "x", "1420.7"], "plugin": ["obfs", "", "~"]}
    parts = []
    for k, vals in keys.items():
        if k in ("type", "server", "port") or R.random() < 0.35:
            parts.append(f"{k}: {pick(vals)}")
    return "{" + ", ".join(parts) + "}"


def clash_doc():
    style = R.random()
    entries = [clash_entry() for _ in range(R.randint(0, 5))]
    if style < 0.1:
        return pick(["proxies: 3", "proxies:\n", "x: 1\nproxies:\n  - a", "proxies: [1, 'x', ~]", "proxies:\n  - {a: 1}\n"])
    if style < 0.4:
        body = "\n".join(f"  - {e}" for e in entries)
        extra = "base: &b {type: trojan, server: m.com, port: 443, password: p}\n" if R.random() < 0.5 else ""
        merge = "\n  - {<<: *b, name: merged}\n  - {<<: [*b, {port: 8443}], name: m2}\n  - {<<: *b, port: 9, name: m3}" if extra else ""
        return f"{extra}port: 7890\nproxies:\n{body}{merge}\nproxy-groups: []\n"
    return "proxies: [" + ", ".join(entries) + "]\n"


def sub_body():
    r = R.random()
    if r < 0.35:
        return clash_doc()
    lines = [link() for _ in range(R.randint(0, 6))] + [pick(["", "# comment", "garbage", "  "]) for _ in range(R.randint(0, 2))]
    R.shuffle(lines)
    text = pick(["\n", "\r\n", "\r", " "]).join(lines)
    if r < 0.75:
        text = b64(text) if R.random() < 0.8 else b64(text)[:-3] + "!!"
        if R.random() < 0.3:
            text = "\n".join(text[i:i + 76] for i in range(0, len(text), 76))
    if R.random() < 0.1:
        text = "﻿" + text
    return text


FIXED = [
    {"kind": "link", "text": "vless://11111111-2222-3333-4444-555555555555@example.com:443?security=reality&pbk=K&sid=ab&sni=www.apple.com&type=grpc&serviceName=svc#My%20Server"},
    {"kind": "link", "text": "hy2://pw@[2001:db8::1]:443,5000-6000/?sni=a.com&obfs=salamander&obfs-password=x"},
    {"kind": "link", "text": "ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#SS"},
    {"kind": "link", "text": "ss://YWVzLTI1Ni1nY206cGFzc0BleGFtcGxlLmNvbTo4Mzg4"},
    {"kind": "link", "text": "tuic://u:p@a.com:443?congestion_control=cubic&alpn=h3&fp=x"},
    {"kind": "link", "text": "ss://rc4-md5:pw@a.com:443"},
    {"kind": "sub", "text": "proxies:\n  - {name: a, type: ss, server: a.com, port: 1, cipher: auto, password: x}\n  - {name: b, type: trojan, server: a.com, port: 1, password: x}"},
]


def client_card():
    t = pick(["socks5", "socks4", "http", "https", "shadowsocks", "hysteria2", "forward", "tun", "wireguard"])
    peers = pick(["[]", json.dumps([{"name": "phone", "private_key": "QL" * 21 + "w=", "public_key": "Pk" * 21 + "w=",
                                     "address": "10.66.0.2/32"}]), "nope"])
    ib = {"type": t, "host": pick(["127.0.0.1", "0.0.0.0", "::", "::1", "192.168.1.9", "my.host"]),
          "port": pick([1080, 443, 8388, 65535]), "auth_enabled": pick([True, False]),
          "username": pick(["u", "user name", "한글"]), "password": pick(["p", "p@ss:w/rd", ""]),
          "ss_method": pick(["aes-256-gcm", "chacha20-ietf-poly1305", "2022-blake3-aes-128-gcm", "2022-blake3-chacha20-poly1305"]),
          "ss_password": pick(["pw", "k/+=", "x y", "한"]), "hy2_password": pick(["pw", "p w", "a@b", "%"]),
          "tls_cert": pick(["", "c.pem"]), "hy2_obfs_password": pick(["", "ob fs", "o"]),
          "wg_peers": peers, "wg_private_key": "SK" * 21 + "w=", "wg_address": "10.66.0.1/24",
          "wg_endpoint": pick(["", "vpn.example"]), "wg_mtu": pick([1408, 1280])}
    return {"name": pick(NAMES_C), "inbound": ib}


NAMES_C = ["", "My card", "한글 카드", "a#b", "x/y?z"]


def python_client(card, lan):
    from proxydock.ui import proxies
    proxies.local_lan_ip = lambda: lan
    return proxies.client_url(card)


def python_side(cases):
    out = []
    for c in cases:
        try:
            if c["kind"] == "client":
                out.append({"ok": True, "url": python_client(c["card"], c["lan"])})
                continue
            if c["kind"] == "link":
                fields, name = links.parse(c["text"])
                out.append({"ok": True, "fields": fields, "name": name})
            else:
                servers, skipped = subscription.parse(c["text"].encode("utf-8"))
                note = i18n._t("{count} servers").format(count=len(servers))
                if skipped:
                    note += " · " + i18n._t("{count} skipped (unsupported or invalid)").format(count=skipped)
                out.append({"ok": True, "sub_servers": json.dumps(servers, ensure_ascii=False), "note": note})
        except ValueError as e:
            out.append({"ok": False, "error": str(e)})
    return out


def main():
    args = sys.argv[1:]
    lang = args[args.index("--lang") + 1] if "--lang" in args else "en"
    seed = int(args[args.index("--seed") + 1]) if "--seed" in args else int(time.time())
    count = int(args[0]) if args and args[0].isdigit() else 500
    R.seed(seed)
    i18n.set_language(lang)
    cases = FIXED + [{"kind": "link", "text": link()} if R.random() < 0.6 else {"kind": "sub", "text": sub_body()}
                     for _ in range(count)]
    # what a client app needs to use a card (ui/proxies.client_url)
    cases += [{"kind": "client", "card": client_card(), "lan": pick(["192.168.0.5", "10.1.2.3"])} for _ in range(count // 2)]
    with contextlib.redirect_stdout(io.StringIO()):
        py = python_side(cases)
    path = os.path.join(tempfile.mkdtemp(), "cases.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(cases, f, ensure_ascii=False)
    exe = os.path.join(ROOT, "native", "target", "debug", "examples", "dump_links.exe")
    rs = json.loads(subprocess.run([exe, path, lang], capture_output=True, check=True, encoding="utf-8").stdout)
    bad = 0
    kinds = {"ok": 0, "err": 0}
    for c, a, b in zip(cases, py, rs):
        kinds["ok" if a["ok"] else "err"] += 1
        if a != b:
            bad += 1
            if bad <= 8:
                print("DIFF", json.dumps(c, ensure_ascii=False)[:600])
                ja, jb = json.dumps(a, ensure_ascii=False), json.dumps(b, ensure_ascii=False)
                i = next((k for k in range(min(len(ja), len(jb))) if ja[k] != jb[k]), min(len(ja), len(jb)))
                print("  py:", ja[max(0, i - 150):i + 250])
                print("  rs:", jb[max(0, i - 150):i + 250])
    print(f"seed {seed}: {len(cases)} cases ({kinds['ok']} read, {kinds['err']} refused), {bad} different")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
