"""Generate a plaintext Bitwarden JSON export for testing.

Deterministic (fixed seed) so two runs produce the same file. Every value is
obviously fake: hosts are under .invalid / .example, which can never resolve,
and no string here is a real credential.
"""
import json, os, random, uuid, base64

random.seed(20260831)

def gid(n):  # deterministic uuids
    return str(uuid.UUID(int=random.getrandbits(128), version=4))

def date(i):
    return "2026-%02d-%02dT%02d:%02d:%02dZ" % (1 + i % 12, 1 + i % 28, i % 24, i % 60, i % 60)

def b32(i):
    raw = ("seed%04d" % i).encode()
    return base64.b32encode(raw).decode().rstrip("=")

FOLDER_NAMES = ["Work", "Personal", "Banking", "Shopping", "Servers",
                "Archive/Old", "Social", "Dev tools", "Family", "Travel",
                "Ünïcøde ✓ folder", "  spaces  "]
folders = [{"id": gid(i), "name": n} for i, n in enumerate(FOLDER_NAMES)]
folder_ids = [f["id"] for f in folders] + [None]

MATCH = [0, 1, 2, 3, 4, 5, None]          # domain host startsWith exact regex never default
FIELD_TYPES = [0, 1, 2, 3]                 # text hidden boolean linked
BRANDS = ["Visa", "Mastercard", "Amex", "Discover", "JCB", "Maestro", "UnionPay", "Other", ""]
EDGE = ["", " ", "a" * 400, "Ünïcøde ✓ ✗ 日本語 emoji 🔐", "<script>alert(1)</script>",
        "line1\nline2\nline3", "tab\there", 'quote"and\backslash', "'; DROP TABLE--", "%s %d {0}"]

def fields(i):
    out = []
    for k in range(i % 5):
        t = FIELD_TYPES[(i + k) % len(FIELD_TYPES)]
        f = {"name": ["Note", "PIN", "Enabled", "Linked", ""][(i + k) % 5],
             "value": None if t == 3 else EDGE[(i + k) % len(EDGE)] if k == 3 else "value-%d-%d" % (i, k),
             "type": t}
        if t == 3:
            f["linkedId"] = 100 + ((i + k) % 2)   # username / password
        out.append(f)
    return out or None

def uris(i):
    n = i % 4
    if n == 0:
        return None
    return [{"match": MATCH[(i + k) % len(MATCH)],
             "uri": ["https://site%d.invalid/login" % i,
                     "http://legacy%d.example/signin?next=/a" % i,
                     "androidapp://com.example.app%d" % i,
                     "iosapp://com.example.app%d" % i][(i + k) % 4]}
            for k in range(n)]

def pw_history(i):
    if i % 7:
        return None
    return [{"lastUsedDate": date(i + k), "password": "old-pass-%d-%d" % (i, k)} for k in range(1 + i % 3)]

def base(i, typ, name):
    return {"id": gid(i), "organizationId": None, "folderId": folder_ids[i % len(folder_ids)],
            "type": typ, "reprompt": i % 2, "name": name, "notes": None if i % 3 == 0 else
            (EDGE[i % len(EDGE)] if i % 11 == 0 else "Notes for item %d." % i),
            "favorite": i % 5 == 0, "fields": fields(i), "collectionIds": None,
            "revisionDate": date(i), "creationDate": date(i), "deletedDate": None}

items = []
i = 0

# ---- logins -------------------------------------------------------------
for n in range(430):
    i += 1
    it = base(i, 1, "Login %04d — %s" % (i, ["bank", "mail", "shop", "forum", "vpn"][i % 5]))
    it["login"] = {
        "uris": uris(i),
        "username": None if i % 13 == 0 else ("user%d@example.invalid" % i if i % 2 else "user_%d" % i),
        "password": None if i % 17 == 0 else (EDGE[i % len(EDGE)] if i % 23 == 0 else "P@ssw0rd!%04d" % i),
        "totp": None if i % 4 else ("otpauth://totp/Example:user%d?secret=%s&issuer=Example&digits=%d&period=%d"
                                    % (i, b32(i), 6 if i % 2 else 8, 30 if i % 3 else 60) if i % 8 else b32(i)),
        "passwordRevisionDate": None if i % 6 else date(i),
        "fido2Credentials": None,
    }
    it["passwordHistory"] = pw_history(i)
    items.append(it)

# ---- secure notes -------------------------------------------------------
for n in range(220):
    i += 1
    it = base(i, 2, "Secure note %04d" % i)
    it["notes"] = EDGE[i % len(EDGE)] if i % 9 == 0 else ("Multi\nline\nnote %d" % i)
    it["secureNote"] = {"type": 0}
    items.append(it)

# ---- cards --------------------------------------------------------------
for n in range(170):
    i += 1
    it = base(i, 3, "Card %04d — %s" % (i, BRANDS[i % len(BRANDS)]))
    it["card"] = {
        "cardholderName": None if i % 12 == 0 else "Test Holder %d" % i,
        "brand": BRANDS[i % len(BRANDS)] or None,
        # 4111... is Visa's published test number; never a real card
        "number": ["4111111111111111", "5555555555554444", "378282246310005",
                   "6011111111111117", ""][i % 5] or None,
        "expMonth": None if i % 10 == 0 else str(1 + i % 12),
        "expYear": None if i % 10 == 0 else str(2027 + i % 6),
        "code": None if i % 8 == 0 else ("%03d" % (i % 1000) if i % 3 else "%04d" % (i % 10000)),
    }
    items.append(it)

# ---- identities ---------------------------------------------------------
for n in range(150):
    i += 1
    it = base(i, 4, "Identity %04d" % i)
    it["identity"] = {
        "title": [None, "Mr", "Mrs", "Ms", "Dr"][i % 5],
        "firstName": "First%d" % i, "middleName": None if i % 3 else "M%d" % i,
        "lastName": "Last%d" % i,
        "address1": "%d Example Street" % i, "address2": None if i % 4 else "Flat %d" % i,
        "address3": None, "city": ["Springfield", "Ünïcøde City", "", "Hill Valley"][i % 4] or None,
        "state": None if i % 5 else "CA", "postalCode": "%05d" % (i % 100000),
        "country": ["US", "GB", "DE", "JP", None][i % 5],
        "company": None if i % 6 else "Example Corp %d" % i,
        "email": "id%d@example.invalid" % i, "phone": "+1-555-%04d" % (i % 10000),
        "ssn": None if i % 7 else "000-00-%04d" % (i % 10000),
        "username": "ident%d" % i,
        "passportNumber": None if i % 9 else "X%07d" % i,
        "licenseNumber": None if i % 11 else "D%08d" % i,
    }
    items.append(it)

# ---- ssh keys -----------------------------------------------------------
for n in range(30):
    i += 1
    it = base(i, 5, "SSH key %04d" % i)
    it["sshKey"] = {
        "privateKey": "-----BEGIN OPENSSH PRIVATE KEY-----\nFAKE%04d\n-----END OPENSSH PRIVATE KEY-----" % i,
        "publicKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5FAKE%04d test%d@example.invalid" % (i, i),
        "keyFingerprint": "SHA256:FAKEfingerprint%04d" % i,
    }
    items.append(it)

doc = {"encrypted": False, "folders": folders, "items": items}
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "deskwarden-test-vault.json")
with open(out, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, indent=2, ensure_ascii=False)

from collections import Counter
c = Counter(x["type"] for x in items)
print("items:", len(items), "folders:", len(folders))
print("by type -> login %d, note %d, card %d, identity %d, ssh %d" % (c[1], c[2], c[3], c[4], c[5]))
print(out)
