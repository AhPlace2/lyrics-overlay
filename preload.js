'use strict';
const { contextBridge, ipcRenderer } = require('electron');

const INCOMING = ['settings', 'state', 'cover', 'lyrics', 'edit', 'toast', 'sync-now', 'panel-open'];
const OUTGOING = ['card-moved', 'edit-done', 'set-offset', 'open-settings', 'ui-hover'];

contextBridge.exposeInMainWorld('overlay', {
  on(channel, fn) {
    if (INCOMING.includes(channel)) ipcRenderer.on(channel, (_event, data) => fn(data));
  },
  send(channel, data) {
    if (OUTGOING.includes(channel)) ipcRenderer.send(channel, data);
  },
});
