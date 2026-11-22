import React, { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import type { CommerceApi, CustomerApi } from '@nexa/contracts';
import { Catalog } from '@nexa/ui';
import { Backoffice } from './backoffice';
import { DesktopUpdater } from './updater';

const api: CommerceApi = {
  listProducts: () => invoke('list_products'),
  searchProducts: (query) => invoke('search_products', { query }),
  getProduct: (productId) => invoke('get_product', { productId }),
  getAvailability: (productId, quantity) => invoke('get_availability', { productId, quantity }),
  getPriceQuote: (productId, currency, channel) => invoke('get_price_quote', { productId, currency, channel }),
};

type SessionStatus = {
  authenticated: boolean;
  operator: boolean;
  expires_at: number | null;
};

function Desktop() {
  const [session, setSession] = useState<SessionStatus | null>(null);
  const customerApi = useMemo<CustomerApi>(() => {
    let latest = 0;
    async function updateSession(command: 'get_session' | 'sign_in' | 'sign_out') {
      const request = ++latest;
      const status = await invoke<SessionStatus>(command);
      if (request === latest) setSession(status);
      return status;
    }

    return {
      signedIn: async () => (await updateSession('get_session')).authenticated,
      signIn: async () => { await updateSession('sign_in'); },
      signOut: async () => { await updateSession('sign_out'); },
      createCart: () => invoke('create_cart'),
      addCartItem: (cartId, productId, quantity) => invoke('add_cart_item', { cartId, productId, quantity }),
    };
  }, []);

  useEffect(() => {
    if (!session?.operator || session.expires_at === null) return;
    const delay = Math.min(2147483647, Math.max(0, session.expires_at * 1000 - Date.now()));
    const timer = window.setTimeout(() => {
      setSession((current) => current === session ? null : current);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [session]);

  return <>
    <Catalog api={api} customerApi={customerApi} title="Nexa Commerce · Desktop (Tauri)" />
    {session?.operator && <Backoffice />}
    <DesktopUpdater />
  </>;
}

createRoot(document.getElementById('root')!).render(
  <React.StrictMode><Desktop /></React.StrictMode>,
);
