import http from 'k6/http';
import { check, sleep } from 'k6';
import { DEFAULT_HEADERS, SLA_OPTIONS } from './common.js';

export const options = SLA_OPTIONS;

const CAL_BASE = __ENV.BASE_URL || 'http://localhost:8002';

export default function () {
  const headers = DEFAULT_HEADERS;

  // List calendars
  const calRes = http.get(`${CAL_BASE}/api/v1/calendars`, { headers });
  check(calRes, {
    'calendars 200': (r) => r.status === 200,
    'calendars latency': (r) => r.timings.duration < 200,
  });

  // List upcoming events (next 7 days)
  const now = new Date().toISOString();
  const week = new Date(Date.now() + 7 * 86400000).toISOString();
  const evtRes = http.get(
    `${CAL_BASE}/api/v1/events?dtstart_gte=${now}&dtstart_lte=${week}&limit=50`,
    { headers }
  );
  check(evtRes, {
    'events 200': (r) => r.status === 200,
    'events latency': (r) => r.timings.duration < 300,
  });

  const healthRes = http.get(`${CAL_BASE}/health`);
  check(healthRes, { 'health 200': (r) => r.status === 200 });

  sleep(1);
}
