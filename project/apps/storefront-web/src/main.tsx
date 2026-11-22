import React from 'react';
import { createRoot } from 'react-dom/client';
import { createWebApi } from '@nexa/api-client';
import { Catalog } from '@nexa/ui';

const api = createWebApi(import.meta.env.VITE_API_BASE_URL ?? 'http://127.0.0.1:8082');

createRoot(document.getElementById('root')!).render(
  <React.StrictMode><Catalog api={api} title="Nexa Commerce · Web Storefront" /></React.StrictMode>,
);
