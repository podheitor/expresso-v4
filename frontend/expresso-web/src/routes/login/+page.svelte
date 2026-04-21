<script lang="ts">
  import { login } from '$lib/stores/auth.svelte';
  import { onMount } from 'svelte';

  let redirect = $state('/');

  onMount(() => {
    const p = new URLSearchParams(window.location.search);
    redirect = p.get('redirect') ?? '/';
  });
</script>

<section class="mx-auto mt-24 max-w-md p-6 rounded-lg border bg-white shadow">
  <h1 class="text-2xl font-semibold mb-2">Entrar no Expresso</h1>
  <p class="text-gray-600 mb-6">Autenticação via provedor OIDC.</p>
  <button
    class="w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700"
    onclick={() => login(redirect)}
  >
    Entrar com Keycloak
  </button>
</section>
