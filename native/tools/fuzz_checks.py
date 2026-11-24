"""Settings files whose cards hit the save / start checks (check_proxy, outbound_errors, wg_server_errors,
match_errors) with edge values, for parity_checks.py.

    python native/tools/fuzz_checks.py <out dir> [count]
"""
import json
import os
import random
import sys

KEY = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk="
B64_16 = "AAAAAAAAAAAAAAAAAAAAAA=="


def pick(*xs):
    return random.choice(xs)


def text_pool():
    return pick("", " ", "x", "abc", "a b", "한글", "12345678", "1234567", KEY, KEY[:-1], B64_16, B64_16[:-2], "abc=",
                "ab==", "a===", "AAAA====", KEY + "=", "#" * 44, "\u00e9" * 8)


def inbound():
    t = pick("socks5", "socks4", "http", "https", "shadowsocks", "hysteria2", "wireguard", "forward", "tun")
    peers = pick("", "[]", "x", json.dumps([{"name": "p", "public_key": KEY, "address": "10.66.0.2/32"}]),
                 json.dumps([{"name": "p", "public_key": "bad", "address": "10.66.0.1/32"},
                             {"public_key": KEY, "address": "10.67.0.2/32"}, {"name": None, "public_key": KEY, "address": "10.66.0.3"},
                             {"name": 5, "public_key": KEY, "address": "10.66.0.3/32"}]))
    return {"type": t, "host": pick("127.0.0.1", "0.0.0.0", "::", "::1", "localhost", "fe80::1%3", "bad host", "", "1.2.3.04"),
            "port": pick(1080, 1080, 8080, 0, 65536, 443), "auth_enabled": pick(True, False),
            "username": pick("", "u", "x" * 256), "password": pick("", "p", "y" * 300),
            "tls_cert": pick("", "C:/nope.crt"), "tls_key": pick("", "C:/nope.key"),
            "ss_method": pick("2022-blake3-aes-128-gcm", "2022-blake3-aes-256-gcm", "aes-128-gcm", "chacha20-ietf-poly1305"),
            "ss_password": text_pool(), "hy2_password": pick("", "short", "longenough"),
            "wg_private_key": pick("", KEY, "bad"), "wg_address": pick("10.66.0.1/24", "10.66.0.1/31", "10.66.0.1", "x", "fd00::1/64", "10.66.0.1/255.255.255.0"),
            "wg_peers": peers, "wg_mtu": pick(1408, 500, 9100),
            "fwd_host": pick("", "10.0.0.5", " "), "fwd_port": pick(0, 22, 70000), "fwd_network": pick("tcp", "udp", "both")}


def outbound(k):
    t = pick("direct", "http", "https", "socks4", "socks5", "shadowsocks", "hysteria2", "wireguard", "vless", "vmess",
             "trojan", "tuic", "subscription", "group", "block")
    return {"type": t, "host": pick("s.example", "127.0.0.1", "localhost", "", " ", "::1"), "port": pick(1080, 8080, 0, 443, 70000),
            "username": pick("", "u"), "password": pick("", "pw"), "bind": pick("", "10.0.0.2", "not-ip", "fe80::1%2"),
            "ss_method": pick("2022-blake3-aes-128-gcm", "aes-256-gcm"), "ss_password": text_pool(),
            "hy2_password": pick("", "hp"), "hy2_ports": pick("", "20000-30000", "bad", "443,5000-6000", "0-5"),
            "hy2_hop_interval": pick(30, 1, 4000), "hy2_up_mbps": pick(0, 50, 200000), "hy2_down_mbps": pick(0, -1),
            "uuid": pick("", "0b5b8d9e-1234-4567-89ab-0123456789ab", "{0b5b8d9e123445 6789ab0123456789ab}", "urn:uuid:0b5b8d9e-1234-4567-89ab-0123456789ab",
                         "0x5b8d9e1234456789ab0123456789ab", "0b5b8d9e_234-4567-89ab-0123456789ab", "zzzz", "0b5b8d9e-1234-4567-89ab-0123456789a"),
            "security": pick("none", "tls", "reality", "bad"), "reality_public_key": pick("", KEY.replace("+", "-").rstrip("="), "short", "é" * 43),
            "reality_short_id": pick("", "ab", "abc", "abcdef0123456789", "abcdef01234567890a", "zz"), "sni": pick("", "x.example"),
            "fingerprint": pick("", "chrome"), "transport": pick("tcp", "ws", "grpc", "h2"),
            "vmess_cipher": pick("auto", "bad"), "vmess_alter_id": pick(0, 70000),
            "wg_private_key": pick("", KEY, "bad"), "wg_peer_public_key": pick("", KEY), "wg_preshared_key": pick("", KEY, "x"),
            "wg_address": pick("10.0.0.2/32", "10.0.0.2", "", "10.0.0.2/32; fd00::2/128", "10.0.0.2/33"),
            "wg_allowed_ips": pick("0.0.0.0/0, ::/0", "", "x/1"), "wg_mtu": pick(1408, 500, 9300),
            "sub_url": pick("", "https://sub.example/x", "ftp://x"), "sub_servers": pick("", "[]", json.dumps([{"name": "a", "outbound": {"type": "direct"}}])),
            "group_test_url": pick("https://www.gstatic.com/generate_204", "ftp://x", ""), "group_interval": pick(180, 5, 5000),
            "group_tolerance": pick(50, -1, 6000)}


def make():
    n_out = random.randint(2, 6)
    outs = [{"id": f"o{k}", "name": pick(f"out{k}", ""), "outbound": outbound(k), "pos": [0, k * 80]} for k in range(n_out)]
    for o in outs:
        if random.random() < 0.3:
            o["via"] = f"o{random.randrange(n_out)}"
        if o["outbound"]["type"] == "group":
            o["members"] = random.sample([x["id"] for x in outs], random.randint(0, n_out))
    ins = []
    for i in range(random.randint(1, 5)):
        ins.append({"id": f"i{i}", "name": pick(f"card{i}", " ", ""), "inbound": inbound(),
                    "overrides": {"dns_server": pick(None, "", "1.1.1.1", "bad://x", "tls://dns.google")},
                    "target": pick(None, *[x["id"] for x in outs], "r0", f"i{random.randrange(5)}")})
    match = {"ip_cidr": pick([], ["10.0.0.0/8"], ["10.0.0.1/255.0.0.0"], ["bad"], ["10.0.0.0/+8"], ["fd00::/8", "1.2.3.4/33"]),
             "port": pick([], ["443"], ["1-2"], ["0"], ["5-3"], ["+80"], ["8_0"], ["x"]),
             "geoip": pick([], ["kr"], ["KR"], ["-x"]), "geosite": pick([], ["google"], ["a/b"])}
    routers = [{"id": "r0", "name": "R", "rules": [{"id": "x0", "name": pick("", "rule"), "match": match, "target": "o0"}], "default": "o1"}]
    return {"version": 3, "global": {}, "graph": {"inbounds": ins, "outbounds": outs, "routers": routers}}


def main():
    out, count = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 200
    os.makedirs(out, exist_ok=True)
    random.seed(7)
    for i in range(count):
        with open(os.path.join(out, f"k{i:03}.json"), "w", encoding="utf-8") as f:
            json.dump(make(), f, ensure_ascii=False)


if __name__ == "__main__":
    main()
