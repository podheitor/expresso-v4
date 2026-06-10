// Expresso service worker — WebPush only (no fetch interception).
// Payloads are small JSON objects: {kind, folder} (see notifications service).
'use strict';

var KIND_LABELS = {
  new_mail: 'Novo e-mail',
  event_created: 'Novo evento na agenda',
  contact_upserted: 'Contato atualizado',
  seat_overage: 'Limite de licenças excedido',
  storage_overage: 'Limite de armazenamento excedido'
};

self.addEventListener('push', function (e) {
  var data = {};
  try { data = e.data ? e.data.json() : {}; } catch (err) { /* non-JSON push */ }
  var body = KIND_LABELS[data.kind] || data.kind || 'Notificação';
  if (data.folder) body += ' em ' + data.folder;
  e.waitUntil(self.registration.showNotification('Expresso', {
    body: body,
    tag: 'expresso-' + (data.kind || 'notif'),
    data: data
  }));
});

self.addEventListener('notificationclick', function (e) {
  e.notification.close();
  var data = e.notification.data || {};
  var url = data.kind === 'new_mail' ? '/mail' : '/';
  e.waitUntil(self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then(function (wins) {
    for (var i = 0; i < wins.length; i++) {
      if ('focus' in wins[i]) { wins[i].navigate(url); return wins[i].focus(); }
    }
    return self.clients.openWindow(url);
  }));
});
