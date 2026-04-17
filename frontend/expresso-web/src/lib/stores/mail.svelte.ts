// Svelte 5 rune-based store for mail state
import { mailApi, type Folder, type MessageListItem, type MessageDetail } from '$lib/api/mail';

// ─── State ────────────────────────────────────────────────────────────────────
export const mailState = $state({
  folders:         [] as Folder[],
  selectedFolder:  'INBOX',
  messages:        [] as MessageListItem[],
  selectedId:      null as string | null,
  detail:          null as MessageDetail | null,
  loading:         false,
  error:           null as string | null,
  composing:       false,
  page:            0,
});

// ─── Actions ─────────────────────────────────────────────────────────────────

export async function loadFolders(): Promise<void> {
  try {
    mailState.folders = await mailApi.getFolders();
  } catch (e) {
    mailState.error = String(e);
  }
}

export async function selectFolder(name: string): Promise<void> {
  mailState.selectedFolder = name;
  mailState.selectedId    = null;
  mailState.detail        = null;
  mailState.page          = 0;
  await loadMessages();
}

export async function loadMessages(): Promise<void> {
  mailState.loading = true;
  mailState.error   = null;
  try {
    mailState.messages = await mailApi.getMessages(
      mailState.selectedFolder,
      mailState.page,
    );
  } catch (e) {
    mailState.error = String(e);
  } finally {
    mailState.loading = false;
  }
}

export async function openMessage(id: string): Promise<void> {
  mailState.selectedId = id;
  mailState.detail     = null;
  try {
    mailState.detail = await mailApi.getMessage(id);
    // Mark as read in local state immediately (optimistic)
    const msg = mailState.messages.find(m => m.id === id);
    if (msg && !msg.flags.includes('\\Seen')) {
      msg.flags = [...msg.flags, '\\Seen'];
      const folder = mailState.folders.find(f => f.name === mailState.selectedFolder);
      if (folder) folder.unseen_count = Math.max(0, folder.unseen_count - 1);
    }
  } catch (e) {
    mailState.error = String(e);
  }
}

export async function deleteSelected(): Promise<void> {
  if (!mailState.selectedId) return;
  await mailApi.deleteMessage(mailState.selectedId);
  mailState.messages  = mailState.messages.filter(m => m.id !== mailState.selectedId);
  mailState.selectedId = null;
  mailState.detail     = null;
}
