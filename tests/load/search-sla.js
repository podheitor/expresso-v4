import http from 'k6/http';
import { check, sleep } from 'k6';
import { DEFAULT_HEADERS, SLA_OPTIONS } from './common.js';

export const options = SLA_OPTIONS;

const SEARCH_BASE = __ENV.BASE_URL || 'http://localhost:8007';

const QUERIES = ['contrato', 'reunião', 'projeto', 'fatura', 'relatório'];

export default function () {
  const headers = DEFAULT_HEADERS;
  const q = QUERIES[Math.floor(Math.random() * QUERIES.length)];

  const res = http.get(
    `${SEARCH_BASE}/api/v1/search?q=${encodeURIComponent(q)}&limit=20`,
    { headers }
  );
  check(res, {
    'search 200': (r) => r.status === 200,
    'search latency p99': (r) => r.timings.duration < 500,
  });

  const healthRes = http.get(`${SEARCH_BASE}/health`);
  check(healthRes, { 'health 200': (r) => r.status === 200 });

  sleep(0.5);
}
