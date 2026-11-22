import type { Availability, Cart, CommerceApi, CustomerApi, PriceQuote, ProductDto } from '@nexa/contracts';
export type { CommerceApi } from '@nexa/contracts';
// Web adapter. Production browser sessions are secured through a BFF cookie and CSRF rules.
export function createWebApi(baseUrl: string): CommerceApi {
  const productsUrl = `${baseUrl}/api/v1/catalog/products`;
  return {
    async listProducts() {
      const response = await fetch(productsUrl);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return (await response.json()) as ProductDto[]; // Validate response with Zod/OpenAPI in production.
    },
    async getProduct(productId) {
      const response = await fetch(`${productsUrl}/${encodeURIComponent(productId)}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return (await response.json()) as ProductDto;
    },
    async getAvailability(productId, quantity) {
      const query = new URLSearchParams({ product_id: productId, quantity: String(quantity) });
      const response = await fetch(`${baseUrl}/api/v1/inventory/availability?${query}`, { cache: 'no-store' });
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return (await response.json()) as Availability;
    },
    async getPriceQuote(productId, currency, channel) {
      const query = new URLSearchParams({ product_id: productId, currency, channel });
      const response = await fetch(`${baseUrl}/api/v1/pricing/quotes?${query}`, { cache: 'no-store' });
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return (await response.json()) as PriceQuote;
    }
  };
}

export function createWebCustomerApi(): CustomerApi {
  let csrfToken: string | undefined;

  async function write(path: string, body?: object) {
    if (!csrfToken) throw new Error('Sign in to continue.');
    const response = await fetch(path, {
      method: 'POST',
      headers: { 'X-CSRF-Token': csrfToken, 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (response.status === 401) csrfToken = undefined;
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response;
  }

  return {
    async signedIn() {
      const response = await fetch('/auth/session', { cache: 'no-store' });
      if (response.status === 401 || response.status === 503) {
        csrfToken = undefined;
        return false;
      }
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const session: { csrf_token: string } = await response.json();
      csrfToken = session.csrf_token;
      return true;
    },
    async signIn() {
      window.location.assign('/auth/login');
    },
    async signOut() {
      await write('/auth/logout');
      csrfToken = undefined;
    },
    async createCart() {
      return (await (await write('/api/v1/carts')).json()) as Cart;
    },
    async addCartItem(cartId, productId, quantity) {
      const response = await write(`/api/v1/carts/${encodeURIComponent(cartId)}/items`, {
        product_id: productId, quantity,
      });
      return (await response.json()) as Cart;
    },
  };
}
