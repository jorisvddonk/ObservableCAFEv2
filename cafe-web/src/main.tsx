import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { configure, getToken } from 'cafe-web-sdk';
import { App } from './App';

const apiUrl = window.__CAFE_API_URL__ || `http://${window.location.hostname}:4000`;
const savedToken = (() => { try { return localStorage.getItem('cafe_token') || ''; } catch { return ''; } })();
configure(apiUrl, savedToken);
console.log('[main.tsx] booting, apiUrl=', apiUrl);

const root = document.getElementById('root');
if (!root) throw new Error('No #root element found');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
