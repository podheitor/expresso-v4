import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
    plugins: [sveltekit()],
    server: {
        port: 5173,
        proxy: {
            '/api/mail':     'http://localhost:8001',
            '/api/calendar': 'http://localhost:8002',
            '/api/contacts': 'http://localhost:8003',
            '/api/drive':    'http://localhost:8004',
            '/api/auth':     'http://localhost:8080',
        }
    },
    test: {
        include: ['src/**/*.{test,spec}.{ts,js}'],
    },
});
