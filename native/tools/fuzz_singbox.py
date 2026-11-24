"""Random settings files for comparing the sing-box configs (parity_singbox.py): every inbound and outbound type
with its options (TLS / REALITY / transports, Hysteria2 obfs / Brutal / port hopping, WireGuard both ways, TUN
with Fake-IP, subscriptions, groups in every mode), routers with all condition kinds, allowed domains, private
address blocking and DNS servers.

    python native/tools/fuzz_singbox.py <out dir> [count]
"""
import json
import os
import random
import sys

WG_KEY = "YNXtAzepDqRv9H52osJVDQnznT5AL11eQr2Pjk2sAWk="


def pick(*xs):
    return random.choice(xs)


def outbound(k):
    t = pick("direct", "http", "https", "socks4", "socks5", "shadowsocks", "hysteria2", "wireguard", "vless", "vmess",
             "trojan", "tuic", "subscription", "group", "block")
    ob = {"type": t, "host": pick("s.example", "10.0.0.5", "::1"), "port": 1000 + k, "username": pick("", "user"),
          "password": pick("", "pw"), "tls_verify": pick(True, False), "sni": pick("", "sni.example"),
          "ss_method": pick("2022-blake3-aes-256-gcm", "aes-128-gcm"), "ss_password": "c3M=",
          "hy2_password": "hp", "hy2_obfs_password": pick("", "ob"), "hy2_up_mbps": pick(0, 50), "hy2_down_mbps": pick(0, 200),
          "hy2_ports": pick("", "20000-30000", "443,5000-6000", "bad", " 1 - 2 "), "hy2_hop_interval": pick(30, 5, 0),
          "bind": pick("", "192.168.0.2", "fe80::1%3", "not-ip"), "interface": pick("", "wg0", "이더넷"),
          "wg_private_key": WG_KEY, "wg_address": pick("10.0.0.2/32", "10.0.0.2/32; fd00::2/128"), "wg_peer_public_key": WG_KEY,
          "wg_preshared_key": pick("", WG_KEY), "wg_allowed_ips": "0.0.0.0/0, ::/0", "wg_mtu": pick(1408, 1280), "wg_keepalive": pick(0, 25),
          "uuid": " 0b5b8d9e-1111-2222-3333-444455556666 ", "security": pick("tls", "none", "reality"),
          "vless_flow": pick("", "xtls-rprx-vision"), "vmess_cipher": pick("auto", "none"), "vmess_alter_id": pick(0, 4),
          "reality_public_key": " pk ", "reality_short_id": pick("", "ab"), "fingerprint": pick("chrome", ""),
          "alpn": pick("", "h2, http/1.1", "h3"), "transport": pick("tcp", "ws", "grpc"), "transport_path": pick("", "/p", "svc"),
          "transport_host": pick("", "cdn.example"), "tuic_congestion": pick("bbr", "cubic"), "tuic_udp_relay": pick("native", "quic"),
          "sub_url": "https://sub.example/x", "sub_mode": pick("auto", "select"), "sub_selected": pick("", "A", "zz"),
          "sub_test_url": pick("", "https://t.example"),
          "group_mode": pick("auto", "failover", "select"), "group_test_url": pick("", "https://g.example"),
          "group_interval": pick(180, 5, 60), "group_tolerance": pick(50, 0), "group_selected": pick("", "o0", "o1")}
    if t == "subscription":
        servers = []
        for j in range(random.randint(0, 4)):
            s = {"name": pick("A", "B", f"S{j}"), "outbound": pick(
                {"type": "shadowsocks", "host": "x", "port": 1, "ss_method": "aes-128-gcm", "ss_password": "p"},
                {"type": "trojan", "host": "y", "port": 2, "password": "p"},
                {"type": "vless", "host": "z", "port": 3},                  # no uuid: skipped (KeyError)
                {"type": "snell", "host": "w", "port": 4},                  # unsupported: skipped
                {"type": "hysteria2", "host": "h", "port": 5, "hy2_password": "p", "sni": "h"})}
            servers.append(s)
        ob["sub_servers"] = json.dumps(servers)
    return ob


MATCHES = [{"domain": ["a.example."]}, {"domain_suffix": ["*.kr."]}, {"domain_keyword": ["ads"]},
           {"ip_cidr": ["10.1.2.3/8", "192.168.1.1", "2001:DB8::1/32"]}, {"ip_private": True},
           {"geoip": ["kr", "jp"]}, {"geosite": ["netflix"]}, {"port": ["443", "8000-9000"]}, {"process_name": ["game.exe"]},
           {"port": ["x"]}, {"ip_cidr": ["10.0.0.0/33"]}, {}]


def main():
    out, count = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 300
    os.makedirs(out, exist_ok=True)
    random.seed(7)
    for i in range(count):
        ob_ids = [f"o{k}" for k in range(random.randint(1, 5))]
        rt_ids = [f"r{k}" for k in range(random.randint(0, 2))]
        targets = ob_ids + rt_ids
        outbounds = []
        for k, oid in enumerate(ob_ids):
            n = {"id": oid, "outbound": outbound(k), "via": pick(None, None, *ob_ids)}
            if n["outbound"]["type"] == "group":
                n["members"] = random.sample(ob_ids, k=random.randint(1, len(ob_ids)))
            outbounds.append(n)
        routers = [{"id": rid, "rules": [{"id": f"{rid}{j}", "name": f"R{j}",
                                          "match": dict(random.choice(MATCHES), **(random.choice(MATCHES) if random.random() < 0.3 else {})),
                                          "target": random.choice(targets)} for j in range(random.randint(1, 3))],
                    "default": random.choice(targets + [None]), "style": pick("router", "if"), "switch": random.random() < 0.2,
                    "switch_on": random.random() < 0.5} for rid in rt_ids]
        inbounds = []
        for k in range(random.randint(1, 3)):
            t = pick("socks5", "socks4", "http", "https", "shadowsocks", "hysteria2", "wireguard", "forward", "tun")
            ib = {"type": t, "host": pick("127.0.0.1", "0.0.0.0", "localhost", "::", "192.168.0.10"), "port": 3000 + k,
                  "auth_enabled": pick(True, False), "username": "u", "password": "p", "ss_password": "c3M=",
                  "hy2_password": "hp", "hy2_obfs_password": pick("", "o"), "hy2_up_mbps": pick(0, 10), "hy2_down_mbps": pick(0, 20),
                  "tun_stack": pick("mixed", "gvisor"), "tun_strict_route": pick(True, False), "tun_fakeip": pick(True, False),
                  "wg_private_key": WG_KEY, "wg_address": "10.66.0.1/24",
                  "wg_peers": json.dumps([{"name": "phone", "public_key": " " + WG_KEY + " ", "address": "10.66.0.2/32"}]),
                  "wg_mtu": pick(1408, 0), "fwd_host": pick("[::1]", " 10.0.0.9 "), "fwd_port": 22,
                  "fwd_network": pick("tcp", "udp", "both")}
            overrides = {"allowed_domains": pick(None, ["*"], ["Example.com.", "*.kr", "10.0.0.1", "::1"]),
                         "block_private": pick(None, True, False), "timeout": pick(None, 30),
                         "dns_server": pick(None, "", "1.1.1.1", "tls://dns.google", "https://dns.example/q", "h3://1.1.1.1:8443"),
                         "dns_via_outbound": pick(None, True)}
            inbounds.append({"id": f"i{k}", "inbound": ib, "overrides": overrides, "target": random.choice(targets + [None])})
        glob = {"allowed_domains": pick(["*"], ["a.example"]), "block_private": pick(True, False), "timeout": pick(120, 60),
                "dns_server": pick("", "8.8.8.8"), "dns_via_outbound": pick(True, False)}
        with open(os.path.join(out, f"s{i:03}.json"), "w", encoding="utf-8") as f:
            json.dump({"version": 3, "global": glob,
                       "graph": {"outbounds": outbounds, "inbounds": inbounds, "routers": routers}}, f, ensure_ascii=False)
    print(f"{count} files in {out}")


if __name__ == "__main__":
    main()
