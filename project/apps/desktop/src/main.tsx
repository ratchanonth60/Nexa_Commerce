import React from 'react';
import { createRoot } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import type { CommerceApi, CustomerApi } from '@nexa/contracts';
import { Catalog } from '@nexa/ui';

const api: CommerceApi = {
  listProducts: () => invoke('list_products'),
  getProduct: (productId) => invoke('get_product', { productId }),
  getAvailability: (productId, quantity) => invoke('get_availability', { productId, quantity }),
  getPriceQuote: (productId, currency, channel) => invoke('get_price_quote', { productId, currency, channel }),
};

const customerApi: CustomerApi = {
  signedIn: async () => (await invoke<{ authenticated: boolean }>('get_session')).authenticated,
  signIn: async () => { await invoke('sign_in'); },
  signOut: async () => { await invoke('sign_out'); },
  createCart: () => invoke('create_cart'),
  addCartItem: (cartId, productId, quantity) => invoke('add_cart_item', { cartId, productId, quantity }),
};

createRoot(document.getElementById('root')!).render(
  <React.StrictMode><Catalog api={api} customerApi={customerApi} title="Nexa Commerce · Desktop (Tauri)" /></React.StrictMode>,
);
