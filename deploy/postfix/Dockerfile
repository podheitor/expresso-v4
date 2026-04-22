# Postfix MTA — terminates :25/:465/:587, relays inbound to expresso-mail LMTP :24
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      postfix ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy config templates + entrypoint
COPY main.cf.tmpl  /etc/postfix/main.cf.tmpl
COPY master.cf     /etc/postfix/master.cf
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 25 465 587

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
