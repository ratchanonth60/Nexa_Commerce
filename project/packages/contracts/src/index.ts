// In production, generate these DTOs from the versioned OpenAPI document.
export type ProductDto = {
  id: string;
  sku: string;
  title: string;
  status: 'draft' | 'active' | 'archived';
};

export type CommerceApi = {
  listProducts(): Promise<ProductDto[]>;
  getProduct(productId: string): Promise<ProductDto | null>;
};
