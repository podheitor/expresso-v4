#!/bin/bash
set -euo pipefail

: "${MAIL_DOMAIN:?MAIL_DOMAIN env required}"
: "${LMTP_HOST:=expresso-mail}"
: "${LMTP_PORT:=24}"
MILTER_HOST="${MILTER_HOST:-}"
MILTER_PORT="${MILTER_PORT:-8891}"

sed -e "s|__MAIL_DOMAIN__|${MAIL_DOMAIN}|g" \
    -e "s|__LMTP_HOST__|${LMTP_HOST}|g" \
    -e "s|__LMTP_PORT__|${LMTP_PORT}|g" \
    /etc/postfix/main.cf.tmpl > /etc/postfix/main.cf

if [[ -n "$MILTER_HOST" ]]; then
  MILTER_BLOCK="smtpd_milters = inet:${MILTER_HOST}:${MILTER_PORT}
non_smtpd_milters = inet:${MILTER_HOST}:${MILTER_PORT}
milter_default_action = accept
milter_protocol = 6
milter_mail_macros = i {mail_addr} {client_addr} {client_name} {auth_authen}
milter_rcpt_macros = i {rcpt_addr}"
else
  MILTER_BLOCK="# milter disabled"
fi
awk -v repl="$MILTER_BLOCK" '{gsub(/__MILTER_CONFIG__/, repl)}1' \
    /etc/postfix/main.cf > /etc/postfix/main.cf.new && mv /etc/postfix/main.cf.new /etc/postfix/main.cf

# Ensure postfix spool dir exists before Dovecot creates socket inside it
postfix set-permissions 2>/dev/null || true
rm -f /var/spool/postfix/private/auth
mkdir -p /var/spool/postfix/private
chown postfix:postfix /var/spool/postfix/private

# Start Dovecot (SASL-only) in background
echo "[entrypoint] starting dovecot (SASL only)…"
dovecot -F &
DOVE_PID=$!
sleep 1
# Verify socket ready
if [[ ! -S /var/spool/postfix/private/auth ]]; then
  echo "[entrypoint] ERROR: dovecot auth socket not created"
  exit 1
fi

postfix check
echo "=== main.cf rendered ==="
grep -vE '^\s*#|^\s*$' /etc/postfix/main.cf | head -40

# Trap signals → propagate to dovecot
trap 'echo "[entrypoint] shutting down"; kill $DOVE_PID 2>/dev/null; postfix stop; exit 0' TERM INT

exec postfix start-fg
