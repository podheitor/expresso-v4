# Postfix MTA + Dovecot (SASL-only, Keycloak-backed) + TLS
FROM debian:trixie-slim
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      postfix ca-certificates \
      dovecot-core \
      curl \
    && rm -rf /var/lib/apt/lists/*

COPY main.cf.tmpl         /etc/postfix/main.cf.tmpl
COPY master.cf            /etc/postfix/master.cf
COPY entrypoint.sh        /usr/local/bin/entrypoint.sh
COPY dovecot-auth.conf    /etc/dovecot/dovecot.conf
COPY kc-verify.sh         /usr/local/bin/kc-verify.sh
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/kc-verify.sh

EXPOSE 25 465 587
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
