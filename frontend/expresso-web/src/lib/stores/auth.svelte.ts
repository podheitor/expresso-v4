// Svelte 5 rune-based auth store. Call `loadMe()` on layout mount.
import { authApi, type MeResponse } from '$lib/api/auth';

export const authState = $state({
  user:    null as MeResponse | null,
  loading: true,
  error:   null as string | null,
});

export async function loadMe(): Promise<void> {
  authState.loading = true;
  authState.error   = null;
  try {
    authState.user = await authApi.me();
  } catch (e) {
    authState.error = String(e);
    authState.user  = null;
  } finally {
    authState.loading = false;
  }
}

export function login(redirect = '/'): void {
  window.location.href = authApi.loginUrl(redirect);
}

export function logout(): void {
  window.location.href = authApi.logoutUrl();
}
