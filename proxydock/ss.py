"""Shadowsocks (AEAD-2018 / 2022) encryption, shared by server and client. Network code is in inbound/outbound."""
from .i18n import _t
import base64
import hashlib
import os
import secrets
import struct
import time
import urllib.parse

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM, ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

from .common import APP_NAME, encode_address, parse_address

# --- Shadowsocks ---
# AEAD-2018 (https://shadowsocks.org/doc/aead.html)
#  key    = EVP_BytesToKey(password, MD5), session subkey = HKDF-SHA1(key, salt, "ss-subkey")
#  TCP    = [salt][encrypted length(2)+tag][encrypted data+tag]... (chunks up to 0x3FFF, nonce +1 each time)
#  UDP    = [salt][encrypted (address + data)+tag] (nonce 0)
# 2022 (SIP022, https://shadowsocks.org/doc/sip022.html)
#  key    = base64 PSK as is, session subkey = BLAKE3.derive_key("shadowsocks 2022 session subkey", PSK + salt)
#  TCP    = [salt][fixed header: type, timestamp, variable header length][variable header: address, padding, first data][chunks (up to 0xFFFF)]...
#           the response carries the request salt in its fixed header. Timestamp within ±30 s; reused salts rejected for 60 s.
#  UDP    = AES: [AES-ECB(PSK, session id + packet id)][AEAD(session subkey, nonce=header[4:16])]
#           ChaCha: [24-byte nonce][XChaCha20-Poly1305(PSK, session id + packet id + body)]
#  address = SOCKS5 address format (ATYP + address + port)

SS_TAG = 16
SS_MAX_CHUNK = 0x3FFF       # AEAD-2018
SS22_MAX_CHUNK = 0xFFFF     # 2022
SS22_TIME_WINDOW = 30       # 2022 timestamp tolerance (s)
SS22_SALT_TTL = 60          # 2022 salt reuse rejection window (s)
SS_UDP_IDLE = 120           # UDP session idle expiry (s)
SS_MAX_UDP_SESSIONS = 1024
SS_MAX_PADDING = 900

SS_METHODS = {
    # name: (key length, AEAD class, is 2022)
    "2022-blake3-aes-128-gcm": (16, AESGCM, True),
    "2022-blake3-aes-256-gcm": (32, AESGCM, True),
    "2022-blake3-chacha20-poly1305": (32, ChaCha20Poly1305, True),
    "chacha20-ietf-poly1305": (32, ChaCha20Poly1305, False),
    "aes-256-gcm": (32, AESGCM, False),
    "aes-128-gcm": (16, AESGCM, False),
}


# --- BLAKE3 (for 2022 key derivation; inputs up to 1024 bytes = a single chunk only) ---

_B3_IV = (0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19)
_B3_PERM = (2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)
_B3_CHUNK_START, _B3_CHUNK_END, _B3_ROOT = 1, 2, 8
_B3_DERIVE_CONTEXT, _B3_DERIVE_MATERIAL = 32, 64
_M32 = 0xFFFFFFFF


def _b3_compress(cv, m, counter, block_len, flags):
    v = list(cv) + list(_B3_IV[:4]) + [counter & _M32, (counter >> 32) & _M32, block_len, flags]
    m = list(m)

    def g(a, b, c, d, x, y):
        v[a] = (v[a] + v[b] + x) & _M32
        t = v[d] ^ v[a]
        v[d] = ((t >> 16) | (t << 16)) & _M32
        v[c] = (v[c] + v[d]) & _M32
        t = v[b] ^ v[c]
        v[b] = ((t >> 12) | (t << 20)) & _M32
        v[a] = (v[a] + v[b] + y) & _M32
        t = v[d] ^ v[a]
        v[d] = ((t >> 8) | (t << 24)) & _M32
        v[c] = (v[c] + v[d]) & _M32
        t = v[b] ^ v[c]
        v[b] = ((t >> 7) | (t << 25)) & _M32

    for r in range(7):
        g(0, 4, 8, 12, m[0], m[1])
        g(1, 5, 9, 13, m[2], m[3])
        g(2, 6, 10, 14, m[4], m[5])
        g(3, 7, 11, 15, m[6], m[7])
        g(0, 5, 10, 15, m[8], m[9])
        g(1, 6, 11, 12, m[10], m[11])
        g(2, 7, 8, 13, m[12], m[13])
        g(3, 4, 9, 14, m[14], m[15])
        if r < 6:
            m = [m[i] for i in _B3_PERM]
    return [v[i] ^ v[i + 8] for i in range(8)] + [v[i + 8] ^ cv[i] for i in range(8)]


def _b3_single_chunk(data, key_words, flags, out_len):
    if len(data) > 1024 or out_len > 64:
        raise ValueError(_t("BLAKE3: only inputs up to 1024 bytes and outputs up to 64 bytes are supported"))
    blocks = [data[i:i + 64] for i in range(0, len(data), 64)] or [b""]
    cv = list(key_words)
    for i, block in enumerate(blocks):
        last = i == len(blocks) - 1
        f = flags | (_B3_CHUNK_START if i == 0 else 0) | ((_B3_CHUNK_END | _B3_ROOT) if last else 0)
        words = struct.unpack("<16I", block.ljust(64, b"\0"))
        out = _b3_compress(cv, words, 0, len(block), f)
        if last:
            return struct.pack("<16I", *out)[:out_len]
        cv = out[:8]


def blake3_hash(data, out_len=32):
    return _b3_single_chunk(data, _B3_IV, 0, out_len)


def blake3_derive_key(context, material, out_len):
    ctx_key = _b3_single_chunk(context.encode(), _B3_IV, _B3_DERIVE_CONTEXT, 32)
    return _b3_single_chunk(material, struct.unpack("<8I", ctx_key), _B3_DERIVE_MATERIAL, out_len)


# --- XChaCha20-Poly1305 (2022 chacha UDP) = HChaCha20 subkey + ChaCha20-Poly1305 ---

def _hchacha20(key, nonce16):
    s = [0x61707865, 0x3320646E, 0x79622D32, 0x6B206574] + list(struct.unpack("<8I", key)) \
        + list(struct.unpack("<4I", nonce16))

    def qr(a, b, c, d):
        s[a] = (s[a] + s[b]) & _M32
        t = s[d] ^ s[a]
        s[d] = ((t << 16) | (t >> 16)) & _M32
        s[c] = (s[c] + s[d]) & _M32
        t = s[b] ^ s[c]
        s[b] = ((t << 12) | (t >> 20)) & _M32
        s[a] = (s[a] + s[b]) & _M32
        t = s[d] ^ s[a]
        s[d] = ((t << 8) | (t >> 24)) & _M32
        s[c] = (s[c] + s[d]) & _M32
        t = s[b] ^ s[c]
        s[b] = ((t << 7) | (t >> 25)) & _M32

    for _ in range(10):
        qr(0, 4, 8, 12)
        qr(1, 5, 9, 13)
        qr(2, 6, 10, 14)
        qr(3, 7, 11, 15)
        qr(0, 5, 10, 15)
        qr(1, 6, 11, 12)
        qr(2, 7, 8, 13)
        qr(3, 4, 9, 14)
    return struct.pack("<8I", *(s[0:4] + s[12:16]))


def xchacha_encrypt(key, nonce24, data):
    return ChaCha20Poly1305(_hchacha20(key, nonce24[:16])).encrypt(b"\0" * 4 + nonce24[16:], data, None)


def xchacha_decrypt(key, nonce24, data):
    return ChaCha20Poly1305(_hchacha20(key, nonce24[:16])).decrypt(b"\0" * 4 + nonce24[16:], data, None)


# --- keys ---

def ss_master_key(password, key_len):
    """AEAD-2018: OpenSSL EVP_BytesToKey(MD5, 1 iteration, no salt)."""
    pw = password.encode("utf-8")
    out, prev = b"", b""
    while len(out) < key_len:
        prev = hashlib.md5(prev + pw).digest()
        out += prev
    return out[:key_len]


def ss_make_key(method, password):
    """Build the server key from the configured password. 2022 needs a base64 PSK (ValueError otherwise)."""
    key_len, _, v2022 = SS_METHODS[method]
    if not v2022:
        return ss_master_key(password, key_len)
    try:
        psk = base64.b64decode(password.strip(), validate=True)
    except (ValueError, TypeError):
        raise ValueError(_t("The 2022 password must be a base64 key (use the 'Generate' button)"))
    if len(psk) != key_len:
        raise ValueError(_t("The 2022 key must be {key_len} bytes (currently {len} bytes)").format(key_len=key_len, len=len(psk)))
    return psk


def ss_generate_password(method):
    key_len, _, v2022 = SS_METHODS[method]
    return base64.b64encode(os.urandom(key_len)).decode() if v2022 else secrets.token_urlsafe(18)


def _ss_aead(method, key, salt):
    key_len, cls, v2022 = SS_METHODS[method]
    if v2022:
        sub = blake3_derive_key("shadowsocks 2022 session subkey", key + salt, key_len)
    else:
        sub = HKDF(algorithm=hashes.SHA1(), length=key_len, salt=salt, info=b"ss-subkey").derive(key)
    return cls(sub)


class _SaltFilter:
    """Replay protection: remembers recently seen salts, only for ttl seconds when ttl is given."""

    def __init__(self, cap=100_000, ttl=None):
        self.cap, self.ttl = cap, ttl
        self.seen = {}

    def add(self, salt):
        now = time.monotonic()
        t = self.seen.get(salt)
        if t is not None and (self.ttl is None or now - t < self.ttl):
            return False
        if len(self.seen) >= self.cap:
            if self.ttl is not None:
                self.seen = {k: v for k, v in self.seen.items() if now - v < self.ttl}
            if len(self.seen) >= self.cap:
                for k in list(self.seen)[: self.cap // 2]:
                    del self.seen[k]
        self.seen[salt] = now
        return True


_ss_salts = _SaltFilter()
_ss22_salts = _SaltFilter(ttl=SS22_SALT_TTL)


def _check_ss22_time(ts):
    if abs(time.time() - ts) > SS22_TIME_WINDOW:
        raise ValueError(_t("2022 clock difference over 30 seconds (check the PC clock)"))


class SSDecryptor:
    """AEAD-2018 TCP stream decryption. The buffer only advances a read offset and is compacted once at the end."""

    MAX_CHUNK = SS_MAX_CHUNK

    def __init__(self, method, key):
        self.method, self.key = method, key
        self.salt_len = SS_METHODS[method][0]
        self.aead = None
        self.counter = 0
        self.buf = bytearray()
        self.need = None            # length of the next data chunk (None = waiting for the length block)
        self.salt = None

    def _nonce(self):
        n = self.counter.to_bytes(12, "little")
        self.counter += 1
        return n

    def _open(self, buf, pos, n):
        return self.aead.decrypt(self._nonce(), bytes(buf[pos:pos + n]), None)

    def _on_salt(self, salt):
        if not _ss_salts.add(salt):
            raise ValueError(_t("salt reused (possible replay attack)"))

    def _header(self, buf, pos, out):
        """For 2022 request header parsing. (new offset, header complete)"""
        return pos, True

    def feed(self, data):
        """Feed ciphertext, get back the plaintext recovered so far. InvalidTag/ValueError on authentication failure."""
        buf = self.buf
        buf += data
        pos = 0
        out = []
        try:
            if self.aead is None:
                if len(buf) < self.salt_len:
                    return b""
                self.salt = bytes(buf[: self.salt_len])
                pos = self.salt_len
                self._on_salt(self.salt)
                self.aead = _ss_aead(self.method, self.key, self.salt)
            pos, ready = self._header(buf, pos, out)
            if not ready:
                return b"".join(out)
            size = len(buf)
            while True:
                if self.need is None:
                    if size - pos < 2 + SS_TAG:
                        break
                    ln = self._open(buf, pos, 2 + SS_TAG)
                    pos += 2 + SS_TAG
                    self.need = ((ln[0] << 8) | ln[1]) & self.MAX_CHUNK
                n = self.need + SS_TAG
                if size - pos < n:
                    break
                out.append(self._open(buf, pos, n))
                pos += n
                self.need = None
            return b"".join(out)
        finally:
            if pos:
                del buf[:pos]


class SS22Decryptor(SSDecryptor):
    """2022 TCP request decryption. Only the address + first data of the variable header come out as plaintext (padding removed)."""

    MAX_CHUNK = SS22_MAX_CHUNK
    FIXED = 1 + 8 + 2

    def __init__(self, method, key):
        super().__init__(method, key)
        self.state = "fixed"
        self.var_len = 0

    def _on_salt(self, salt):
        if not _ss22_salts.add(salt):
            raise ValueError(_t("salt reused (possible replay attack)"))

    def _header(self, buf, pos, out):
        if self.state == "fixed":
            if len(buf) - pos < self.FIXED + SS_TAG:
                return pos, False
            hdr = self._open(buf, pos, self.FIXED + SS_TAG)
            pos += self.FIXED + SS_TAG
            typ, ts, self.var_len = struct.unpack("!BQH", hdr)
            if typ != 0:
                raise ValueError(_t("2022 invalid request type"))
            _check_ss22_time(ts)
            self.state = "var"
        if self.state == "var":
            if len(buf) - pos < self.var_len + SS_TAG:
                return pos, False
            var = self._open(buf, pos, self.var_len + SS_TAG)
            pos += self.var_len + SS_TAG
            _, _, end = parse_address(var, 0)
            if len(var) < end + 2:
                raise ValueError(_t("2022 variable header too short"))
            pad = struct.unpack("!H", var[end:end + 2])[0]
            if pad > SS_MAX_PADDING or len(var) < end + 2 + pad:
                raise ValueError(_t("2022 invalid padding length"))
            out.append(var[:end] + var[end + 2 + pad:])
            self.state = "data"
        return pos, True


class SSEncryptor:
    """AEAD-2018 TCP stream encryption."""

    MAX_CHUNK = SS_MAX_CHUNK

    def __init__(self, method, key):
        self.salt = os.urandom(SS_METHODS[method][0])
        self.aead = _ss_aead(method, key, self.salt)
        self.prefix = self.salt
        self.counter = 0

    def _nonce(self):
        n = self.counter.to_bytes(12, "little")
        self.counter += 1
        return n

    def _first(self, data, parts):
        return data

    def encrypt(self, data):
        parts = []
        if self.prefix:
            parts.append(self.prefix)
            self.prefix = b""
            data = self._first(data, parts)
        enc = self.aead.encrypt
        step = self.MAX_CHUNK
        for i in range(0, len(data), step):
            chunk = data[i: i + step]
            parts.append(enc(self._nonce(), len(chunk).to_bytes(2, "big"), None))
            parts.append(enc(self._nonce(), chunk, None))
        return b"".join(parts)


class SS22Encryptor(SSEncryptor):
    """2022 TCP response encryption: the first data goes out with the fixed header carrying the request salt."""

    MAX_CHUNK = SS22_MAX_CHUNK

    def __init__(self, method, key, request_salt):
        super().__init__(method, key)
        self.request_salt = request_salt

    def _first(self, data, parts):
        first, rest = data[:self.MAX_CHUNK], data[self.MAX_CHUNK:]
        hdr = struct.pack("!BQ", 1, int(time.time())) + self.request_salt + struct.pack("!H", len(first))
        parts.append(self.aead.encrypt(self._nonce(), hdr, None))
        parts.append(self.aead.encrypt(self._nonce(), first, None))
        return rest


_SS_ZERO_NONCE = bytes(12)


def ss_udp_decrypt(method, key, packet):
    """AEAD-2018 UDP."""
    salt_len = SS_METHODS[method][0]
    if len(packet) < salt_len + SS_TAG + 1:
        raise ValueError("too short")
    return _ss_aead(method, key, packet[:salt_len]).decrypt(_SS_ZERO_NONCE, packet[salt_len:], None)


def ss_udp_encrypt(method, key, plain):
    salt = os.urandom(SS_METHODS[method][0])
    return salt + _ss_aead(method, key, salt).encrypt(_SS_ZERO_NONCE, plain, None)



class _ReplayWindow:
    """2022 UDP packet id replay protection (sliding window of 1024)."""
    SIZE = 1024

    def __init__(self):
        self.top = -1
        self.bits = 0

    def check(self, pid):
        if pid > self.top:
            shift = pid - self.top
            self.bits = ((self.bits << shift) | 1) & ((1 << self.SIZE) - 1) if shift < self.SIZE else 1
            self.top = pid
            return True
        off = self.top - pid
        if off >= self.SIZE or (self.bits >> off) & 1:
            return False
        self.bits |= 1 << off
        return True



def ss_link(host, port, method, password, name=APP_NAME):
    """SIP002 ss:// link. 2022 percent-encodes the user info instead of using base64."""
    if SS_METHODS[method][2]:
        user = urllib.parse.quote(method, safe="") + ":" + urllib.parse.quote(password, safe="")
    else:
        user = base64.urlsafe_b64encode(f"{method}:{password}".encode()).decode().rstrip("=")
    h = f"[{host}]" if ":" in host else host
    return f"ss://{user}@{h}:{port}#{urllib.parse.quote(name, safe='')}"




# --- client role (outbound Shadowsocks tunnel) ---

class SSClientDecryptor(SSDecryptor):
    """AEAD-2018 response decryption (salts sent by the server are not added to the replay filter)."""

    def _on_salt(self, salt):
        pass


class SS22ClientDecryptor(SSDecryptor):
    """2022 response decryption: [salt][fixed header: type 1, timestamp, request salt, first chunk length][first chunk][chunks]..."""

    MAX_CHUNK = SS22_MAX_CHUNK

    def __init__(self, method, key, request_salt):
        super().__init__(method, key)
        self.request_salt = request_salt
        self.header_done = False

    def _on_salt(self, salt):
        pass

    def _header(self, buf, pos, out):
        if self.header_done:
            return pos, True
        n = 1 + 8 + self.salt_len + 2 + SS_TAG
        if len(buf) - pos < n:
            return pos, False
        hdr = self._open(buf, pos, n)
        pos += n
        typ, ts = struct.unpack("!BQ", hdr[:9])
        if typ != 1:
            raise ValueError(_t("2022 invalid response type"))
        _check_ss22_time(ts)
        if hdr[9:9 + self.salt_len] != self.request_salt:
            raise ValueError(_t("2022 request salt mismatch in response"))
        self.need = struct.unpack("!H", hdr[-2:])[0]     # length of the first data chunk (comes directly, without a length block)
        self.header_done = True
        return pos, True


class SS22ClientEncryptor(SSEncryptor):
    """2022 request encryption: the first send carries the fixed header + variable header (address, padding, first data)."""

    MAX_CHUNK = SS22_MAX_CHUNK

    def __init__(self, method, key, target):
        super().__init__(method, key)
        self.target = target            # encode_address(host, port)

    def _first(self, data, parts):
        first, rest = data[:self.MAX_CHUNK - 600], data[self.MAX_CHUNK - 600:]
        pad = b"" if first else os.urandom(secrets.randbelow(SS_MAX_PADDING) + 1)
        var = self.target + struct.pack("!H", len(pad)) + pad + first
        fixed = struct.pack("!BQH", 0, int(time.time()), len(var))
        parts.append(self.aead.encrypt(self._nonce(), fixed, None))
        parts.append(self.aead.encrypt(self._nonce(), var, None))
        return rest


class SS2018ClientEncryptor(SSEncryptor):
    """AEAD-2018 request: the destination address goes in front of the first chunk."""

    def __init__(self, method, key, target):
        super().__init__(method, key)
        self.target = target

    def _first(self, data, parts):
        return self.target + data


def ss_client_codec(method, key, host, port):
    """(encryptor, decryptor factory). The decryptor needs the request salt, so it is built after the encryptor."""
    target = encode_address(host, port)
    if SS_METHODS[method][2]:
        enc = SS22ClientEncryptor(method, key, target)
        return enc, SS22ClientDecryptor(method, key, enc.salt)
    return SS2018ClientEncryptor(method, key, target), SSClientDecryptor(method, key)


class SSUDPClientCodec:
    """Shadowsocks UDP client (outbound). encode(address+data) / decode(packet) -> address+data."""

    def __init__(self, method, key):
        self.method, self.key = method, key
        self.key_len, self.cls, self.v2022 = SS_METHODS[method]
        self.sid = os.urandom(8)
        self.pid = 0
        self.ecb = Cipher(algorithms.AES(key), modes.ECB()) if self.v2022 and self.cls is AESGCM else None
        self.aead = None
        if self.ecb is not None:
            self.aead = self.cls(blake3_derive_key("shadowsocks 2022 session subkey", key + self.sid, self.key_len))
        self.server_aeads = {}

    def encode(self, addr_and_payload):
        if not self.v2022:
            return ss_udp_encrypt(self.method, self.key, addr_and_payload)
        body = struct.pack("!BQH", 0, int(time.time()), 0) + addr_and_payload
        hdr = self.sid + struct.pack("!Q", self.pid)
        self.pid += 1
        if self.ecb is not None:
            e = self.ecb.encryptor()
            return e.update(hdr) + e.finalize() + self.aead.encrypt(hdr[4:16], body, None)
        nonce = os.urandom(24)
        return nonce + xchacha_encrypt(self.key, nonce, hdr + body)

    def decode(self, packet):
        if not self.v2022:
            return ss_udp_decrypt(self.method, self.key, packet)
        if self.ecb is not None:
            d = self.ecb.decryptor()
            hdr = d.update(packet[:16]) + d.finalize()
            ssid = hdr[:8]
            aead = self.server_aeads.get(ssid)
            if aead is None:
                if len(self.server_aeads) > 16:
                    self.server_aeads.clear()
                aead = self.server_aeads[ssid] = self.cls(
                    blake3_derive_key("shadowsocks 2022 session subkey", self.key + ssid, self.key_len))
            body = aead.decrypt(hdr[4:16], packet[16:], None)
        else:
            plain = xchacha_decrypt(self.key, packet[:24], packet[24:])
            body = plain[16:]
        typ, ts = struct.unpack("!BQ", body[:9])
        if typ != 1 or body[9:17] != self.sid:
            raise ValueError("bad 2022 udp response")
        _check_ss22_time(ts)
        pad = struct.unpack("!H", body[17:19])[0]
        return body[19 + pad:]
