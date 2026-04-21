<script lang="ts">
  import '@fontsource/inter/400.css';
  import '@fontsource/inter/500.css';
  import '@fontsource/inter/700.css';
  import '../app.css';

  import { onMount } from 'svelte';
  import { authState, loadMe } from '$lib/stores/auth.svelte';

  // Public routes that ≠ require auth
  const PUBLIC = ['/login', '/auth'];

  onMount(async () => {
    await loadMe();
    const path = window.location.pathname;
    const isPublic = PUBLIC.some((p) => path === p || path.startsWith(p + '/'));
    if (!authState.user && !isPublic) {
      window.location.href = `/login?redirect=${encodeURIComponent(path)}`;
    }
  });
</script>

<slot />
