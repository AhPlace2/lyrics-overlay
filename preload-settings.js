'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('panel', {
  onState(fn) { ipcRenderer.on('panel-state', (_event, data) => fn(data)); },
  onOpening(fn) { ipcRenderer.on('panel-opening', () => fn()); },
  onClosing(fn) { ipcRenderer.on('panel-closing', () => fn()); },
  act(name, value) { ipcRenderer.send('panel-action', { name, value }); },
});
