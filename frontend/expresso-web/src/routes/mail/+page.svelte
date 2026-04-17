<script lang="ts">
  import { mailState, openMessage, deleteSelected } from '$lib/stores/mail.svelte';

  // Format date to readable PT-BR short form
  function formatDate(iso: string | null): string {
    if (!iso) return '';
    const d = new Date(iso);
    const now = new Date();
    const sameDay = d.toDateString() === now.toDateString();
    return sameDay
      ? d.toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit' })
      : d.toLocaleDateString('pt-BR', { day: '2-digit', month: 'short' });
  }

  function isUnread(msg: { flags: string[] }): boolean {
    return !msg.flags.includes('\\Seen');
  }
</script>

<!-- Two-pane layout: message list + detail -->
<div class="pane-split">
  <!-- Message list -->
  <section class="msg-list" aria-label="Mensagens">
    <div class="list-toolbar">
      <h1 class="folder-title">{mailState.selectedFolder}</h1>
      {#if mailState.loading}
        <span class="loading-dot" aria-live="polite">Carregando…</span>
      {/if}
    </div>

    {#if mailState.error}
      <div class="error-banner" role="alert">{mailState.error}</div>
    {/if}

    {#if mailState.messages.length === 0 && !mailState.loading}
      <div class="empty-state">Nenhuma mensagem</div>
    {/if}

    {#each mailState.messages as msg (msg.id)}
      <button
        class="msg-row {msg.id === mailState.selectedId ? 'selected' : ''} {isUnread(msg) ? 'unread' : ''}"
        onclick={() => openMessage(msg.id)}
      >
        <div class="msg-meta">
          <span class="msg-from">{msg.from_name ?? msg.from_addr ?? '—'}</span>
          <span class="msg-date">{formatDate(msg.date)}</span>
        </div>

        <div class="msg-subject">{msg.subject ?? '(sem assunto)'}</div>

        {#if msg.preview_text}
          <div class="msg-preview">{msg.preview_text}</div>
        {/if}

        <div class="msg-flags">
          {#if msg.has_attachments}<span title="Anexo">📎</span>{/if}
        </div>
      </button>
    {/each}
  </section>

  <!-- Detail pane -->
  <section class="msg-detail" aria-label="Leitura">
    {#if mailState.detail}
      {@const d = mailState.detail}
      <div class="detail-header">
        <h2 class="detail-subject">{d.subject ?? '(sem assunto)'}</h2>
        <div class="detail-actions">
          <button class="btn-icon" title="Responder">↩</button>
          <button class="btn-icon" title="Encaminhar">→</button>
          <button
            class="btn-icon danger"
            title="Excluir"
            onclick={deleteSelected}
          >🗑</button>
        </div>
      </div>

      <div class="detail-meta">
        <span><strong>De:</strong> {d.from_name ?? ''} &lt;{d.from_addr ?? ''}&gt;</span>
        <span class="detail-date">{formatDate(d.date)}</span>
      </div>

      <div class="detail-body">
        <!-- Body is served from object storage; for now show path as placeholder -->
        <p class="body-placeholder">
          Corpo disponível via: <code>{d.body_path}</code>
        </p>
        {#if d.preview_text}
          <p>{d.preview_text}</p>
        {/if}
      </div>
    {:else}
      <div class="detail-empty">
        <span>Selecione uma mensagem para ler</span>
      </div>
    {/if}
  </section>
</div>

<style>
  .pane-split {
    display: grid;
    grid-template-columns: 360px 1fr;
    height: 100%;
    overflow: hidden;
  }

  /* ── Message list ── */
  .msg-list {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    border-right: 1px solid #e2e6ea;
    background: #fff;
  }
  .list-toolbar {
    display: flex;
    align-items: center;
    gap: .75rem;
    padding: .75rem 1rem;
    border-bottom: 1px solid #e2e6ea;
    position: sticky;
    top: 0;
    background: #fff;
    z-index: 1;
  }
  .folder-title {
    font-size: .9rem;
    font-weight: 700;
    color: var(--color-brand);
    margin: 0;
    text-transform: uppercase;
    letter-spacing: .04em;
  }
  .loading-dot { font-size: .75rem; color: #888; }

  .error-banner {
    margin: .5rem;
    padding: .5rem .75rem;
    background: #fff0f0;
    color: #c00;
    border-radius: var(--radius-md);
    font-size: .8rem;
  }
  .empty-state {
    padding: 2rem;
    text-align: center;
    color: #999;
    font-size: .875rem;
  }

  .msg-row {
    display: flex;
    flex-direction: column;
    gap: .15rem;
    width: 100%;
    padding: .75rem 1rem;
    background: none;
    border: none;
    border-bottom: 1px solid #f0f2f5;
    cursor: pointer;
    text-align: left;
    transition: background .1s;
  }
  .msg-row:hover    { background: #f7f9fc; }
  .msg-row.selected { background: #e8f0fe; }
  .msg-row.unread .msg-from,
  .msg-row.unread .msg-subject { font-weight: 700; }

  .msg-meta {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .msg-from    { font-size: .875rem; color: var(--color-foreground); }
  .msg-date    { font-size: .75rem; color: #888; white-space: nowrap; margin-left: .5rem; }
  .msg-subject { font-size: .8rem; color: var(--color-foreground); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .msg-preview { font-size: .75rem; color: #999; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .msg-flags   { display: flex; gap: .25rem; font-size: .75rem; }

  /* ── Detail pane ── */
  .msg-detail {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    background: #fff;
    padding: 1.5rem;
  }
  .detail-empty {
    margin: auto;
    color: #bbb;
    font-size: .9rem;
    text-align: center;
  }

  .detail-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: .75rem;
  }
  .detail-subject { font-size: 1.15rem; font-weight: 600; margin: 0; }

  .detail-actions { display: flex; gap: .5rem; flex-shrink: 0; }
  .btn-icon {
    font-size: 1rem;
    padding: .35rem .6rem;
    border: 1px solid #dde0e6;
    border-radius: var(--radius-md);
    background: none;
    cursor: pointer;
    transition: background .1s;
  }
  .btn-icon:hover       { background: #f0f2f5; }
  .btn-icon.danger:hover{ background: #fff0f0; }

  .detail-meta {
    display: flex;
    justify-content: space-between;
    font-size: .8rem;
    color: #666;
    padding-bottom: .75rem;
    border-bottom: 1px solid #e2e6ea;
    margin-bottom: 1rem;
  }
  .detail-date { white-space: nowrap; }

  .detail-body { line-height: 1.6; font-size: .9rem; }
  .body-placeholder { font-size: .8rem; color: #888; }
  code { background: #f0f2f5; padding: .1rem .3rem; border-radius: 3px; font-size: .75rem; }
</style>
