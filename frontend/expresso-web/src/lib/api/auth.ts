// Auth REST client — talks to expresso-auth-rp via dev proxy.
// Session is cookie-based (httpOnly expresso_at). Frontend never sees JWT.

export interface MeResponse {
  user_id:       string;
  tenant_id:     string;
  email:         string;
  display_name:  string | null;
  roles:         string[];
  expires_at:    number;
}

export const authApi = {
  async me(): Promise<MeResponse | null> {
    const r = await fetch('/auth/me', { credentials: 'include' });
    if (r.status === 401) return null;
    if (!r.ok) throw new Error(`auth/me failed: ${r.status}`);
    return r.json();
  },

  loginUrl(redirect = '/'): string {
    const q = new URLSearchParams({ redirect_uri: redirect });
    return `/auth/login?${q.toString()}`;
  },

  logoutUrl(): string {
    return '/auth/logout';
  },

  async refresh(): Promise<boolean> {
    const r = await fetch('/auth/refresh', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    });
    return r.ok;
  },
};
