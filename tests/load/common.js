export const BASE_URL = __ENV.BASE_URL || 'http://localhost:8001';
export const TOKEN    = __ENV.TOKEN    || '';

export const DEFAULT_HEADERS = {
  Authorization: `Bearer ${TOKEN}`,
  'Content-Type': 'application/json',
};

export const SLA_THRESHOLDS = {
  http_req_duration: ['p(99)<300', 'p(95)<150'],
  http_req_failed:   ['rate<0.01'],
  checks:            ['rate>0.99'],
};

export const SMOKE_OPTIONS = {
  vus: 1,
  duration: '30s',
  thresholds: SLA_THRESHOLDS,
};

export const SLA_OPTIONS = {
  stages: [
    { duration: '1m',  target: 10  },
    { duration: '3m',  target: 50  },
    { duration: '1m',  target: 0   },
  ],
  thresholds: SLA_THRESHOLDS,
};

export const STRESS_OPTIONS = {
  stages: [
    { duration: '2m',  target: 50  },
    { duration: '3m',  target: 100 },
    { duration: '2m',  target: 200 },
    { duration: '2m',  target: 0   },
  ],
  thresholds: {
    http_req_duration: ['p(99)<1000'],
    http_req_failed:   ['rate<0.05'],
  },
};
