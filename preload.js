'use strict';
const { contextBridge, ipcRenderer } = require('electron');

const INCOMING = ['settings', 'state', 'cover', 'cover-colour', 'lyrics', 'edit', 'toast', 'sync-now', 'panel-open', 'pointer-left', 'peek'];
const OUTGOING = ['card-moved', 'edit-done', 'set-offset', 'open-settings', 'ui-hover', 'control', 'card-rect', 'set'];

contextBridge.exposeInMainWorld('overlay', {
  on(channel, fn) {
    if (INCOMING.includes(channel)) ipcRenderer.on(channel, (_event, data) => fn(data));
  },
  send(channel, data) {
    if (OUTGOING.includes(channel)) ipcRenderer.send(channel, data);
  },
});
