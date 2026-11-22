import type { CommerceApi, ProductDto } from '@nexa/contracts';
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
    }
  };
}
