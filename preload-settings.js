'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('panel', {
  onState(fn) { ipcRenderer.on('panel-state', (_event, data) => fn(data)); },
  act(name, value) { ipcRenderer.send('panel-action', { name, value }); },
});
