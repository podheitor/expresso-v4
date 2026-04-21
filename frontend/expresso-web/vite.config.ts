import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, loadEnv } from 'vite';

// Dev proxy → backend services. Override via .env files:
//   VITE_AUTH_HOST, VITE_CHAT_HOST, VITE_MEET_HOST, VITE_MAIL_HOST
export default defineConfig(({ mode }) => {
    const env  = loadEnv(mode, process.cwd(), '');
    const AUTH = env.VITE_AUTH_HOST ?? 'http://localhost:8012';
    const CHAT = env.VITE_CHAT_HOST ?? 'http://localhost:8010';
    const MEET = env.VITE_MEET_HOST ?? 'http://localhost:8011';
    const MAIL = env.VITE_MAIL_HOST ?? 'http://localhost:8001';

    return {
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
    };
});
