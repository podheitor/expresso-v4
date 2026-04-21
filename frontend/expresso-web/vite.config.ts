import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Dev proxy → backend services running on VM 125 (or localhost).
// Override with VITE_AUTH_HOST / VITE_CHAT_HOST / VITE_MEET_HOST envs
// when backends run on different hosts.
const AUTH = process.env.VITE_AUTH_HOST ?? 'http://localhost:8012';
const CHAT = process.env.VITE_CHAT_HOST ?? 'http://localhost:8010';
const MEET = process.env.VITE_MEET_HOST ?? 'http://localhost:8011';
const MAIL = process.env.VITE_MAIL_HOST ?? 'http://localhost:8001';

export default defineConfig({
    plugins: [sveltekit()],
    server: {
        port: 5173,
        proxy: {
            '/auth':         AUTH,
            '/api/chat':     { target: CHAT, rewrite: (p) => p.replace(/^\/api\/chat/, '/api/v1') },
            '/api/meet':     { target: MEET, rewrite: (p) => p.replace(/^\/api\/meet/, '/api/v1') },
            '/api/mail':     MAIL,
            '/api/calendar': 'http://localhost:8002',
            '/api/contacts': 'http://localhost:8003',
            '/api/drive':    'http://localhost:8004',
        }
    },
    test: {
        include: ['src/**/*.{test,spec}.{ts,js}'],
    },
});
