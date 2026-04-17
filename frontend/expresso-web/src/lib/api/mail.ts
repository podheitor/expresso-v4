// Mail REST API client — typed wrappers around /api/mail/*

export interface Folder {
  id:           string;
  name:         string;
  special_use:  string | null;
  message_count: number;
  unseen_count:  number;
  subscribed:    boolean;
}

export interface MessageListItem {
  id:              string;
  subject:         string | null;
  from_addr:       string | null;
  from_name:       string | null;
  has_attachments: boolean;
  preview_text:    string | null;
  flags:           string[];
  date:            string | null;
  size_bytes:      number;
}

export interface MessageDetail extends MessageListItem {
  mailbox_id:  string;
  to_addrs:    unknown;
  cc_addrs:    unknown;
  reply_to:    string | null;
  message_id:  string | null;
  thread_id:   string | null;
  body_path:   string;
  received_at: string;
}

export interface SendRequest {
  from:       string;
  to:         string[];
  cc?:        string[];
  subject:    string;
  body_text?: string;
  body_html?: string;
}

const BASE = '/api/mail';

async function get<T>(path: string): Promise<T> {
  const res = await fetch(`${BASE}${path}`);
  if (!res.ok) throw new Error(`GET ${path} → ${res.status}`);
  return res.json() as Promise<T>;
}

async function del(path: string): Promise<void> {
  const res = await fetch(`${BASE}${path}`, { method: 'DELETE' });
  if (!res.ok && res.status !== 204) throw new Error(`DELETE ${path} → ${res.status}`);
}

async function patch<B, R = void>(path: string, body: B): Promise<R> {
  const res = await fetch(`${BASE}${path}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok && res.status !== 204) throw new Error(`PATCH ${path} → ${res.status}`);
  return (res.status === 204 ? undefined : res.json()) as Promise<R>;
}

async function post<B>(path: string, body: B): Promise<void> {
  const res = await fetch(`${BASE}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok && res.status !== 202) throw new Error(`POST ${path} → ${res.status}`);
}

export const mailApi = {
  getFolders: () => get<Folder[]>('/v1/mail/folders'),

  getMessages: (folder = 'INBOX', page = 0, limit = 50) =>
    get<MessageListItem[]>(`/v1/mail/messages?folder=${encodeURIComponent(folder)}&page=${page}&limit=${limit}`),

  getMessage: (id: string) => get<MessageDetail>(`/v1/mail/messages/${id}`),

  deleteMessage: (id: string) => del(`/v1/mail/messages/${id}`),

  moveMessage: (id: string, target_folder: string) =>
    patch(`/v1/mail/messages/${id}/move`, { target_folder }),

  setFlags: (id: string, add: string[], remove: string[]) =>
    patch(`/v1/mail/messages/${id}/flags`, { add, remove }),

  sendMessage: (req: SendRequest) => post('/v1/mail/send', req),
};
