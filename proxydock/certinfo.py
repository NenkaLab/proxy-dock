"""Read a server's certificate to fill in the SNI ("Detect from server" in the editor).

The official Hysteria2 server (and many TLS servers) refuse a handshake whose SNI does not match their
certificate. When an outbound points at an IP, the name to use is in the certificate: connect once without
verification, read the certificate's DNS names and its SHA-256 fingerprint.
"""
from .i18n import _t
import asyncio
import hashlib
import socket
import ssl

from .common import is_ip_literal, strip_scope

TIMEOUT = 8
QUIC_TYPES = ("hysteria2", "tuic")


def _sni(ob):
    name = (ob.get("sni") or "").strip() or ob["host"]
    return None if is_ip_literal(name) else name      # never send an IP as SNI


def tcp_certificate(host, port, sni=None):
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    with socket.create_connection((strip_scope(host), port), TIMEOUT) as s:
        with ctx.wrap_socket(s, server_hostname=sni) as t:
            return t.getpeercert(binary_form=True)


def quic_certificate(host, port, sni=None, alpn=("h3",)):
    from aioquic.asyncio import connect
    from aioquic.quic.configuration import QuicConfiguration
    from cryptography.hazmat.primitives import serialization

    async def run():
        cfg = QuicConfiguration(is_client=True, alpn_protocols=list(alpn), verify_mode=ssl.CERT_NONE)
        if sni:
            cfg.server_name = sni
        async with connect(strip_scope(host), port, configuration=cfg, wait_connected=True) as proto:
            certs = proto._quic.tls.peer_certificates
            if not certs:
                raise ValueError(_t("The server sent no certificate"))
            return certs[0].public_bytes(serialization.Encoding.DER)

    return asyncio.run(asyncio.wait_for(run(), TIMEOUT))


def names_of(der):
    """DNS names of a certificate: subjectAltName DNS entries first, then the common name."""
    from cryptography import x509
    from cryptography.x509.oid import ExtensionOID, NameOID
    cert = x509.load_der_x509_certificate(der)
    names = []
    try:
        san = cert.extensions.get_extension_for_oid(ExtensionOID.SUBJECT_ALTERNATIVE_NAME).value
        names += san.get_values_for_type(x509.DNSName)
    except x509.ExtensionNotFound:
        pass
    for attr in cert.subject.get_attributes_for_oid(NameOID.COMMON_NAME):
        if attr.value not in names and not is_ip_literal(attr.value) and "." in attr.value:
            names.append(attr.value)
    return names


def detect(ob):
    """For an outbound: {"names": [...], "sni": best name or "", "sha256": fingerprint}. Raises ValueError."""
    try:
        if ob["type"] in QUIC_TYPES:
            der = quic_certificate(ob["host"], ob["port"], _sni(ob), ["h3"])
        else:
            der = tcp_certificate(ob["host"], ob["port"], _sni(ob))
    except (OSError, asyncio.TimeoutError, ConnectionError, ssl.SSLError) as e:
        raise ValueError(_t("Could not read the certificate: {e}").format(e=e or type(e).__name__)) from None
    names = names_of(der)
    exact = [n for n in names if not n.startswith("*.")]
    return {"names": names, "sni": exact[0] if exact else "", "sha256": hashlib.sha256(der).hexdigest()}
