// In production, generate these DTOs from the versioned OpenAPI document.
export type ProductDto = {
  id: string;
  sku: string;
  title: string;
  status: 'draft' | 'active' | 'archived';
};

export type PriceQuote = {
  quote_id: string;
  product_id: string;
  currency: string;
  unit_amount_minor: string;
  expires_at: string;
  rule_version: number;
};

export type Availability = {
  product_id: string;
  quantity: number;
  available: boolean;
};

export type CommerceApi = {
  listProducts(): Promise<ProductDto[]>;
  getProduct(productId: string): Promise<ProductDto | null>;
  getAvailability(productId: string, quantity: number): Promise<Availability | null>;
  getPriceQuote(productId: string, currency: string, channel: string): Promise<PriceQuote | null>;
};

export type Cart = {
  id: string;
  version: number;
  items: { product_id: string; quantity: number }[];
};

export type CustomerApi = {
  signedIn(): Promise<boolean>;
  signIn(): Promise<void>;
  signOut(): Promise<void>;
  createCart(): Promise<Cart>;
  addCartItem(cartId: string, productId: string, quantity: number): Promise<Cart>;
};
