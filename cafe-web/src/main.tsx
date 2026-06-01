import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';

const root = document.getElementById('root');
if (!root) throw new Error('No #root element found');

console.log('[main.tsx] booting, origin=', window.location.origin);

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
