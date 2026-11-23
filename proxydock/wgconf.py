"""WireGuard helpers: key pairs, key checks and reading a WireGuard .conf file (the format VPN providers hand out)."""
from .i18n import _t
import base64
import ipaddress

DEFAULT_ALLOWED = "0.0.0.0/0, ::/0"
DEFAULT_MTU = 1408


def keypair():
    """(private, public) as base64, like `wg genkey | wg pubkey`."""
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
    k = X25519PrivateKey.generate()
    priv = k.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    return base64.b64encode(priv).decode(), public_of(base64.b64encode(priv).decode())


def public_of(private_key):
    """Public key for a base64 private key ("" if it is not a valid key)."""
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
    try:
        raw = base64.b64decode(private_key.strip(), validate=True)
        k = X25519PrivateKey.from_private_bytes(raw)
    except ValueError:
        return ""
    return base64.b64encode(k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)).decode()


def valid_key(text):
    try:
        return len(base64.b64decode(text.strip(), validate=True)) == 32
    except ValueError:
        return False


def next_peer_address(server_address, used):
    """Next free client address (/32 or /128) in the server's subnet, or None."""
    import ipaddress
    net = ipaddress.ip_interface(server_address.strip())
    taken = {ipaddress.ip_interface(x).ip for x in used} | {net.ip}
    for host in net.network.hosts():
        if host not in taken:
            return f"{host}/{net.max_prefixlen}"
    return None


def client_conf(ib, peer, endpoint_host, dns="1.1.1.1"):
    """WireGuard config text for one device of a ProxyDock WireGuard server (paste or import into its app)."""
    from .common import host_port
    return "\n".join([
        "[Interface]",
        f"PrivateKey = {peer['private_key']}",
        f"Address = {peer['address']}",
        f"DNS = {dns}",
        f"MTU = {ib.get('wg_mtu') or 1408}",
        "",
        "[Peer]",
        f"PublicKey = {public_of(ib['wg_private_key'])}",
        f"Endpoint = {host_port(endpoint_host, ib['port'])}",
        "AllowedIPs = 0.0.0.0/0",
        "PersistentKeepalive = 25",
        ""])


def split(text):
    return [x.strip() for x in text.replace(";", ",").split(",") if x.strip()]


def valid_cidrs(text, need_prefix=True):
    items = split(text)
    if not items:
        return False
    for x in items:
        if need_prefix and "/" not in x:
            return False
        try:
            ipaddress.ip_network(x, strict=False)
        except ValueError:
            return False
    return True


def parse_conf(text):
    """WireGuard .conf -> (outbound fields, notes). Raises ValueError when it is not a usable client config.
    Only the first [Peer] is used; DNS is not used (ProxyDock is not a system VPN)."""
    section, iface, peers = None, {}, []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip().lower()
            if section == "peer":
                peers.append({})
            continue
        if "=" not in line:
            continue
        k, v = (x.strip() for x in line.split("=", 1))
        k = k.lower()
        if section == "interface":
            iface[k] = v if k not in iface else iface[k] + ", " + v
        elif section == "peer" and peers:
            peers[-1][k] = v if k not in peers[-1] else peers[-1][k] + ", " + v
    if "privatekey" not in iface or not peers or "publickey" not in peers[0] or "endpoint" not in peers[0]:
        raise ValueError(_t("Not a WireGuard client config (needs [Interface] PrivateKey and a [Peer] with PublicKey and Endpoint)"))
    peer = peers[0]
    ep = peer["endpoint"]
    if ep.startswith("["):
        host, _, port = ep[1:].partition("]:")
    else:
        host, _, port = ep.rpartition(":")
    if not host or not port.isdigit():
        raise ValueError(_t("Cannot read the Endpoint '{value}'").format(value=ep))
    out = {
        "type": "wireguard", "host": host, "port": int(port),
        "wg_private_key": iface["privatekey"], "wg_address": iface.get("address", ""),
        "wg_peer_public_key": peer["publickey"], "wg_preshared_key": peer.get("presharedkey", ""),
        "wg_allowed_ips": peer.get("allowedips", DEFAULT_ALLOWED),
        "wg_mtu": int(iface["mtu"]) if iface.get("mtu", "").isdigit() else DEFAULT_MTU,
        "wg_keepalive": int(peer["persistentkeepalive"]) if peer.get("persistentkeepalive", "").isdigit() else 0,
    }
    notes = []
    if len(peers) > 1:
        notes.append(_t("The file has {count} peers; only the first is used").format(count=len(peers)))
    if iface.get("dns"):
        notes.append(_t("DNS from the file is not used (domains are resolved by ProxyDock)"))
    return out, notes
