import React from 'react';
import { createRoot } from 'react-dom/client';
import { createWebApi, createWebCustomerApi } from '@nexa/api-client';
import { Catalog } from '@nexa/ui';

const api = createWebApi(import.meta.env.VITE_API_BASE_URL ?? '');
const customerApi = createWebCustomerApi(import.meta.env.VITE_CHECKOUT_SANDBOX === 'true');

createRoot(document.getElementById('root')!).render(
  <React.StrictMode><Catalog api={api} customerApi={customerApi} title="Nexa Commerce · Web Storefront" /></React.StrictMode>,
);
