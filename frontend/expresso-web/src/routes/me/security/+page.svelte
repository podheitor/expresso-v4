<script lang="ts">
  import { authState, loadMe } from '$lib/stores/auth.svelte';
  import { onMount } from 'svelte';

  // Keycloak exposes a self-service account console that handles all WebAuthn
  // credential lifecycle (register, rename, delete). We link into the tenant
  // realm's credential tab so admins don't need to re-implement enrolment UX.
  const KC_ACCOUNT = '/auth/realms/expresso/account/#/security/signingin';

  onMount(loadMe);
</script>

<section class="mx-auto mt-12 max-w-2xl p-6 space-y-6">
  <h1 class="text-2xl font-semibold">Segurança da conta</h1>

  {#if authState.loading}
    <p>Carregando…</p>
  {:else if !authState.user}
    <p class="text-red-600">Não autenticado.</p>
    <a class="text-blue-600 underline" href="/login">Ir para login</a>
  {:else}
    <article class="rounded border border-gray-200 p-4">
      <h2 class="text-lg font-semibold mb-2">Autenticação multifator</h2>
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
        <dt class="font-medium">TOTP</dt>
        <dd>
          {#if authState.user.mfa?.totp}
            <span class="text-green-700">Habilitado</span>
          {:else}
            <span class="text-gray-500">Não habilitado</span>
          {/if}
        </dd>

        <dt class="font-medium">Chave de segurança (WebAuthn)</dt>
        <dd>
          {#if authState.user.mfa?.webauthn}
            <span class="text-green-700">Usado nesta sessão</span>
          {:else}
            <span class="text-gray-500">Não usado nesta sessão</span>
          {/if}
        </dd>

        <dt class="font-medium">AMR</dt>
        <dd class="font-mono">{authState.user.mfa?.amr?.join(', ') || '—'}</dd>
        <dt class="font-medium">ACR</dt>
        <dd class="font-mono">{authState.user.mfa?.acr ?? '—'}</dd>
      </dl>
    </article>

    <article class="rounded border border-gray-200 p-4 space-y-3">
      <h2 class="text-lg font-semibold">Registrar chave de segurança</h2>
      <p class="text-sm text-gray-700">
        O registro e gestão de chaves FIDO2/WebAuthn é feito no console de
        autoatendimento do Keycloak. Clique abaixo para abrir a aba de
        credenciais e cadastrar uma nova passkey ou chave física.
      </p>
      <a
        class="inline-block bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700"
        href={KC_ACCOUNT}
        target="_blank"
        rel="noopener"
      >
        Abrir console de credenciais
      </a>
    </article>

    <p class="text-sm">
      <a class="text-blue-600 underline" href="/me">← Voltar ao perfil</a>
    </p>
  {/if}
</section>
