import {AsyncLocalStorage} from 'node:async_hooks';

// Process-local host authority; no request JSON or model string can mint it.
const activeEditor = new AsyncLocalStorage();
export const getEditorInvocation = () => activeEditor.getStore();
export const withEditorInvocation = (authority, run) => activeEditor.run(authority, run);
