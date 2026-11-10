/** Starts the app's interface in the main page. */

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import type { StartupOptions } from '../protocol';
import { App } from './App';
import './app.css';

export function mountApp(options: StartupOptions): void {
  let host = document.getElementById('app');
  if (host === null) {
    host = document.createElement('div');
    host.id = 'app';
    document.body.append(host);
  }
  document.body.classList.add('app-mode');
  createRoot(host).render(
    <StrictMode>
      <App options={options} />
    </StrictMode>,
  );
}
