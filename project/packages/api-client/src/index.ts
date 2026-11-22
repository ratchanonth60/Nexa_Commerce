import type { Availability, CommerceApi, PriceQuote, ProductDto } from '@nexa/contracts';
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
