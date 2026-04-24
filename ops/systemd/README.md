# systemd units for Expresso ops

## expresso-smoke-multirealm

Periodic end-to-end smoke: issues pilot JWT → `/auth/me` → Prometheus query.
Runs every 10 min. Failures → journal (`journalctl -u expresso-smoke-multirealm`).

### Install (prod 125)

```bash
sudo install -m 755 ops/smoke-multirealm.sh /opt/expresso/smoke-multirealm.sh
sudo install -m 644 ops/systemd/expresso-smoke-multirealm.{service,timer} /etc/systemd/system/
sudo install -d -m 750 /etc/expresso
# populate with pilot credentials (one per line, KEY=VALUE)
sudo tee /etc/expresso/smoke-multirealm.env >/dev/null <<ENV
PILOT_REALM=<uuid>
PILOT_CLIENT_SECRET=<secret>
PILOT_PASS=<password>
ENV
sudo chmod 600 /etc/expresso/smoke-multirealm.env
sudo systemctl daemon-reload
sudo systemctl enable --now expresso-smoke-multirealm.timer
```

### Operations

- `systemctl list-timers expresso-smoke-multirealm.timer` — next run
- `systemctl start expresso-smoke-multirealm.service` — run now
- `journalctl -u expresso-smoke-multirealm -n 50` — recent outputs
- Failures: exit code non-zero → journal records; SMOKE FAIL string grep-able

### Future

- Push `expresso_smoke_multirealm_success` gauge to Prometheus via node_exporter textfile collector → enables alerting on stale/failed smoke.
