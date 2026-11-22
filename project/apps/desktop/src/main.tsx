import React from 'react';
import { createRoot } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import type { CommerceApi } from '@nexa/contracts';
import { Catalog } from '@nexa/ui';

const api: CommerceApi = {
  listProducts: () => invoke('list_products'),
  getProduct: (productId) => invoke('get_product', { productId }),
};

createRoot(document.getElementById('root')!).render(
  <React.StrictMode><Catalog api={api} title="Nexa Commerce · Desktop (Tauri)" /></React.StrictMode>,
);
