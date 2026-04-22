#!/bin/bash
set -euo pipefail

: "${MAIL_DOMAIN:?MAIL_DOMAIN env required}"
: "${LMTP_HOST:=expresso-mail}"
: "${LMTP_PORT:=24}"
MILTER_HOST="${MILTER_HOST:-}"
MILTER_PORT="${MILTER_PORT:-8891}"
SASL_USER="${SASL_USER:-}"
SASL_PASS="${SASL_PASS:-}"

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
  MILTER_BLOCK="# milter disabled (MILTER_HOST unset)"
fi

awk -v repl="$MILTER_BLOCK" '{gsub(/__MILTER_CONFIG__/, repl)}1' \
    /etc/postfix/main.cf > /etc/postfix/main.cf.new && mv /etc/postfix/main.cf.new /etc/postfix/main.cf

# Seed sasldb2 if credentials provided
if [[ -n "$SASL_USER" && -n "$SASL_PASS" ]]; then
  echo "[entrypoint] seeding sasldb2 user=${SASL_USER}"
  echo -n "$SASL_PASS" | saslpasswd2 -c -p -u "$MAIL_DOMAIN" "$SASL_USER"
  chown postfix:postfix /etc/sasldb2 || true
  chmod 640 /etc/sasldb2 || true
fi

postfix set-permissions || true
postfix check
echo "=== main.cf rendered ==="
grep -vE '^\s*#|^\s*$' /etc/postfix/main.cf
exec postfix start-fg
