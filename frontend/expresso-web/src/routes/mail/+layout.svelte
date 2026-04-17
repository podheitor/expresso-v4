<script lang="ts">
  import { onMount } from 'svelte';
  import { mailState, loadFolders, selectFolder } from '$lib/stores/mail.svelte';

  onMount(() => { loadFolders(); selectFolder('INBOX'); });

  // Icon map for special folders
  const folderIcon: Record<string, string> = {
    '\\Inbox':  '📥',
    '\\Sent':   '📤',
    '\\Drafts': '📝',
    '\\Trash':  '🗑️',
    '\\Junk':   '🚫',
  };
</script>

<div class="mail-shell">
  <!-- Sidebar -->
  <aside class="sidebar">
    <header class="sidebar-header">
      <span class="app-name">✉ Expresso Mail</span>
    </header>

    <button
      class="compose-btn"
      onclick={() => mailState.composing = true}
    >
      + Compor
    </button>

    <nav class="folder-list" aria-label="Pastas">
      {#each mailState.folders as folder (folder.id)}
        <button
          class="folder-item {mailState.selectedFolder === folder.name ? 'active' : ''}"
          onclick={() => selectFolder(folder.name)}
          aria-current={mailState.selectedFolder === folder.name ? 'page' : undefined}
        >
          <span class="folder-icon">{folderIcon[folder.special_use ?? ''] ?? '📁'}</span>
          <span class="folder-name">{folder.name}</span>
          {#if folder.unseen_count > 0}
            <span class="badge">{folder.unseen_count}</span>
          {/if}
        </button>
      {/each}
    </nav>
  </aside>

  <!-- Main content — injected by child routes -->
  <main class="mail-main">
    {@render children?.()}
  </main>
</div>

<style>
  .mail-shell {
    display: grid;
    grid-template-columns: 220px 1fr;
    height: 100vh;
    overflow: hidden;
    background: var(--color-surface);
  }

  /* Sidebar */
  .sidebar {
    display: flex;
    flex-direction: column;
    background: var(--color-brand);
    color: #fff;
    overflow-y: auto;
  }
  .sidebar-header {
    padding: 1rem 1.25rem 0.5rem;
    font-weight: 700;
    font-size: 1.1rem;
    border-bottom: 1px solid rgba(255,255,255,.15);
  }
  .app-name { letter-spacing: -.01em; }

  .compose-btn {
    margin: 0.75rem 1rem;
    padding: 0.55rem 1rem;
    border-radius: var(--radius-md);
    background: var(--color-accent);
    color: #fff;
    font-weight: 600;
    font-size: .875rem;
    cursor: pointer;
    border: none;
    text-align: left;
    transition: opacity .15s;
  }
  .compose-btn:hover { opacity: .88; }

  .folder-list { flex: 1; padding: 0.25rem 0; }

  .folder-item {
    display: flex;
    align-items: center;
    gap: .5rem;
    width: 100%;
    padding: .55rem 1.25rem;
    background: none;
    border: none;
    color: rgba(255,255,255,.85);
    font-size: .875rem;
    cursor: pointer;
    text-align: left;
    border-radius: 0;
    transition: background .1s;
  }
  .folder-item:hover        { background: rgba(255,255,255,.1); }
  .folder-item.active       { background: rgba(255,255,255,.18); color: #fff; font-weight: 600; }
  .folder-icon              { font-size: 1rem; flex-shrink: 0; }
  .folder-name              { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .badge {
    background: var(--color-accent);
    color: #fff;
    border-radius: 999px;
    padding: 1px 7px;
    font-size: .75rem;
    font-weight: 700;
  }

  /* Main area */
  .mail-main {
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
</style>
