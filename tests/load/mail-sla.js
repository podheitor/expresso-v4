import http from 'k6/http';
import { check, sleep } from 'k6';
import { BASE_URL, DEFAULT_HEADERS, SLA_OPTIONS } from './common.js';

export const options = SLA_OPTIONS;

const MAIL_BASE = __ENV.BASE_URL || 'http://localhost:8001';

export default function () {
  const headers = DEFAULT_HEADERS;

  // List mailboxes
  const listRes = http.get(`${MAIL_BASE}/api/v1/mailboxes`, { headers });
  check(listRes, {
    'mailboxes 200': (r) => r.status === 200,
    'mailboxes latency': (r) => r.timings.duration < 200,
  });

  // List messages in INBOX
  const msgRes = http.get(`${MAIL_BASE}/api/v1/mailboxes/INBOX/messages?limit=20`, { headers });
  check(msgRes, {
    'messages 200': (r) => r.status === 200,
    'messages latency': (r) => r.timings.duration < 300,
  });

  // Health
  const healthRes = http.get(`${MAIL_BASE}/health`);
  check(healthRes, { 'health 200': (r) => r.status === 200 });

  sleep(1);
}
