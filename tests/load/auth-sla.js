import http from 'k6/http';
import { check, sleep } from 'k6';
import { SLA_OPTIONS } from './common.js';

export const options = SLA_OPTIONS;

const AUTH_BASE = __ENV.BASE_URL || 'http://localhost:8100';

export default function () {
  // Health (unauthenticated — tests proxy overhead)
  const healthRes = http.get(`${AUTH_BASE}/health`);
  check(healthRes, {
    'health 200': (r) => r.status === 200,
    'health latency': (r) => r.timings.duration < 100,
  });

  // OIDC discovery endpoint (public, cached by clients)
  const discoveryRes = http.get(`${AUTH_BASE}/.well-known/openid-configuration`);
  check(discoveryRes, {
    'discovery 200': (r) => r.status === 200,
    'discovery latency': (r) => r.timings.duration < 150,
  });

  sleep(1);
}
