import http from 'k6/http';
import { check, sleep } from 'k6';
import { DEFAULT_HEADERS, SLA_OPTIONS } from './common.js';

export const options = SLA_OPTIONS;

const DRIVE_BASE = __ENV.BASE_URL || 'http://localhost:8004';

export default function () {
  const headers = DEFAULT_HEADERS;

  // List root files
  const listRes = http.get(`${DRIVE_BASE}/api/v1/files?parent=root&limit=50`, { headers });
  check(listRes, {
    'files list 200': (r) => r.status === 200,
    'files list latency': (r) => r.timings.duration < 200,
  });

  // Search files
  const searchRes = http.get(`${DRIVE_BASE}/api/v1/files?q=test&limit=20`, { headers });
  check(searchRes, {
    'files search 200': (r) => r.status === 200,
    'files search latency': (r) => r.timings.duration < 300,
  });

  const healthRes = http.get(`${DRIVE_BASE}/health`);
  check(healthRes, { 'health 200': (r) => r.status === 200 });

  sleep(1);
}
