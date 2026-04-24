# systemd units for Expresso ops

## expresso-smoke-multirealm (single tenant, legacy)

Fixed-env unit for pilot1. Reads `/etc/expresso/smoke-multirealm.env`.

## expresso-smoke-multirealm@TENANT (template, multi-tenant)

Instanced unit. Each enabled instance reads `/etc/expresso/smoke-multirealm-<TENANT>.env`.
Random 0-60s jitter avoids push collisions.

### Install (prod 125)

```bash
sudo install -m 755 ops/smoke-multirealm.sh /opt/expresso/smoke-multirealm.sh
sudo install -m 644 ops/systemd/expresso-smoke-multirealm@.service /etc/systemd/system/
sudo install -m 644 ops/systemd/expresso-smoke-multirealm@.timer   /etc/systemd/system/

# Per-tenant env (one file per tenant)
sudo install -d -m 750 /etc/expresso
sudo tee /etc/expresso/smoke-multirealm-pilot.env >/dev/null <<ENV
PILOT_REALM=30aa38fd-5948-47f0-9e42-eee64a621745
PILOT_CLIENT_SECRET=<secret>
PILOT_PASS=<password>
PILOT_USER=admin@pilot.expresso.local
TENANT_HOST=pilot.expresso.local
PUSHGATEWAY_URL=http://127.0.0.1:9091
ENV
sudo chmod 600 /etc/expresso/smoke-multirealm-pilot.env

sudo tee /etc/expresso/smoke-multirealm-pilot2.env >/dev/null <<ENV
PILOT_REALM=3b11c7a2-44d1-4935-963b-ba622b70786a
PILOT_CLIENT_SECRET=<secret>
PILOT_PASS=<password>
PILOT_USER=admin@pilot2.expresso.local
TENANT_HOST=pilot2.expresso.local
PUSHGATEWAY_URL=http://127.0.0.1:9091
ENV
sudo chmod 600 /etc/expresso/smoke-multirealm-pilot2.env

sudo systemctl daemon-reload
sudo systemctl enable --now expresso-smoke-multirealm@pilot.timer
sudo systemctl enable --now expresso-smoke-multirealm@pilot2.timer
```

### Operations

- `systemctl list-timers 'expresso-smoke-multirealm@*.timer'` — all instances
- `systemctl start expresso-smoke-multirealm@pilot.service` — run now
- `journalctl -u 'expresso-smoke-multirealm@*' -n 100`
- `curl -s http://127.0.0.1:9091/metrics | grep expresso_smoke` — all tenants

### Migration from legacy

```bash
sudo systemctl disable --now expresso-smoke-multirealm.timer
# Keep legacy unit for a while → rollback safety, remove later.
```
