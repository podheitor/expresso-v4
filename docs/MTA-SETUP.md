# MTA Setup — Postfix + LMTP + indymilter (Opção A)

Production architecture for Expresso v4 mail pipeline.

```
                 ┌────────────────────────────────────────────┐
  Internet       │                                            │
    │            │              VM / host                     │
    ▼            │                                            │
  :25 SMTP ─────▶│ postfix container ──lmtp──▶ expresso-mail  │
  :465 SMTPS ──▶ │   │                        :24  (LMTP)     │
  :587 SUBM ───▶ │   │                         │              │
                 │   ├── milter → expresso-milter :8891       │
                 │   │   (SPF/DKIM/DMARC verify + A-R inject) │
                 │   │                                        │
                 │   └── (AUTH submission) → sign DKIM → relay│
                 │                                            │
                 └────────────────────────────────────────────┘
```

## Components

| Service | Port(s) | Purpose |
|---|---|---|
| `postfix` | 25, 465, 587 | MTA entry — terminates SMTP, writes queue, delivers via LMTP |
| `expresso-milter` | 8891 | Rust milter sidecar — adds `Authentication-Results`, signs outbound DKIM |
| `expresso-mail` | 24 (LMTP), 8001 (HTTP) | Application — LMTP listener ingests + stores messages, REST API |

## Files

- `deploy/postfix/Dockerfile` — postfix image
- `deploy/postfix/main.cf.tmpl` — main.cf template (rendered via entrypoint)
- `deploy/postfix/master.cf` — service definitions (smtp/submission/smtps)
- `deploy/postfix/entrypoint.sh` — renders template + starts postfix foreground
- `Dockerfile.milter` — expresso-milter image
- `services/expresso-milter/` — Rust crate (indymilter 0.3)
- `services/expresso-mail/src/lmtp.rs` — LMTP RFC 2033 listener

## Postfix env vars

| Var | Default | Description |
|---|---|---|
| `MAIL_DOMAIN` | *(required)* | myhostname + virtual_mailbox_domains |
| `LMTP_HOST` | `expresso-mail` | LMTP delivery host |
| `LMTP_PORT` | `24` | LMTP delivery port |
| `MILTER_HOST` | *(unset = disabled)* | Milter hostname |
| `MILTER_PORT` | `8891` | Milter port |

## Milter env vars

| Var | Default | Description |
|---|---|---|
| `MILTER_ADDR` | `0.0.0.0:8891` | Listen socket |
| `MAIL_DOMAIN` | *(empty)* | `auth-serv-id` in Authentication-Results |
| `DKIM_SELECTOR` | *(empty)* | (TODO) selector name |
| `DKIM_KEY_PATH` | *(empty)* | (TODO) RSA key PEM |

## Compose snippet

```yaml
  postfix:
    build: { context: ., dockerfile: deploy/postfix/Dockerfile }
    container_name: expresso-postfix
    restart: unless-stopped
    environment:
      MAIL_DOMAIN: expresso.local
      LMTP_HOST: expresso-mail
      LMTP_PORT: "24"
      MILTER_HOST: expresso-milter
      MILTER_PORT: "8891"
    ports:
      - "25:25"
      - "465:465"
      - "587:587"
    depends_on: [expresso-mail, expresso-milter]

  expresso-milter:
    build: { context: ., dockerfile: Dockerfile.milter }
    container_name: expresso-milter
    restart: unless-stopped
    environment:
      MILTER_ADDR: "0.0.0.0:8891"
      MAIL_DOMAIN: expresso.local
      # Future — production DKIM sign:
      # DKIM_SELECTOR: default
      # DKIM_KEY_PATH: /run/secrets/dkim.key
```

## Current state (2026-04-22)

### ✅ Complete
- LMTP listener in `expresso-mail` (port 24) — RFC 2033, per-recipient replies, reuses `ingest::process`
- Postfix container config + entrypoint
- Milter scaffold — negotiates `ADD_HEADER`, injects stub `Authentication-Results` on EOM
- Dockerfiles for both services

### ⏳ TODO
- **Milter inbound verification**: integrate `mail-auth` crate to run SPF (`spf.verify(ip, helo, mail_from)`) + DKIM (`dkim.verify(body)`) + DMARC (`dmarc.verify(...)`) → real `Authentication-Results` value
- **Milter outbound DKIM signing**: detect AUTH session via `{auth_authen}` macro → load key from `DKIM_KEY_PATH` → sign body via `mail-auth::dkim::sign` → inject `DKIM-Signature` header using `insert_header`
- **DNS records** required for MX + SPF + DKIM + DMARC (see below)
- **Postfix TLS certs**: mount Let's Encrypt certs; set `smtpd_tls_cert_file` / `smtpd_tls_key_file`
- **SASL auth**: integrate with expresso-auth for SMTP submission

## DNS records (example for `expresso.local` → `mx.expresso.local`)

```dns
; Inbound routing
expresso.local.      IN MX   10 mx.expresso.local.
mx.expresso.local.   IN A    203.0.113.10

; SPF — authorize MX to send
expresso.local.      IN TXT  "v=spf1 mx -all"

; DKIM — public key (companion private key loaded by milter / expresso-mail)
default._domainkey.expresso.local. IN TXT  "v=DKIM1; k=rsa; p=MIIBIjANBgkqhki..."

; DMARC — policy
_dmarc.expresso.local. IN TXT  "v=DMARC1; p=reject; rua=mailto:dmarc@expresso.local"
```

Generate DKIM key via `scripts/dkim-keygen.sh default expresso.local`.

## Testing

Internal loop (no real DNS needed):
```bash
# Start postfix + milter + mail
docker compose up -d postfix expresso-milter expresso-mail

# Send test message via telnet
docker exec -i expresso-postfix bash <<'SH'
printf 'HELO client\nMAIL FROM:<a@ext>\nRCPT TO:<b@expresso.local>\nDATA\nSubject: test\n\nhi\n.\nQUIT\n' | nc localhost 25
SH

# Check mail arrived
docker exec expresso-mail ls /var/lib/expresso/mail/
```

## References

- RFC 2033 — LMTP
- RFC 5321 — SMTP
- RFC 8617 — Authentication-Results (`A-R` header format)
- indymilter 0.3 — https://docs.rs/indymilter
- mail-auth — https://docs.rs/mail-auth
