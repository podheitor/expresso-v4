<script lang="ts">
  import { authState, loadMe, logout } from '$lib/stores/auth.svelte';
  import { onMount } from 'svelte';
  onMount(loadMe);
</script>

<section class="mx-auto mt-12 max-w-2xl p-6">
  {#if authState.loading}
    <p>Carregando…</p>
  {:else if !authState.user}
    <p class="text-red-600">Não autenticado.</p>
    <a class="text-blue-600 underline" href="/login">Ir para login</a>
  {:else}
    <h1 class="text-2xl font-semibold mb-4">Sessão atual</h1>
    <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
      <dt class="font-medium">Usuário</dt>        <dd>{authState.user.display_name ?? '—'}</dd>
      <dt class="font-medium">Email</dt>          <dd>{authState.user.email}</dd>
      <dt class="font-medium">User ID</dt>        <dd class="font-mono">{authState.user.user_id}</dd>
      <dt class="font-medium">Tenant</dt>         <dd class="font-mono">{authState.user.tenant_id}</dd>
      <dt class="font-medium">Roles</dt>          <dd>{authState.user.roles.join(', ') || '—'}</dd>
      <dt class="font-medium">Expira (epoch)</dt> <dd>{authState.user.expires_at}</dd>
    </dl>
    <button
      class="mt-6 bg-gray-200 px-4 py-2 rounded hover:bg-gray-300"
      onclick={logout}
    >
      Sair
    </button>
    <p class="mt-6 text-sm">
      <a class="text-blue-600 underline" href="/me/security">Gerenciar segurança / MFA</a>
    </p>
  {/if}
</section>
