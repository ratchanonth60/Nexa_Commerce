import { useEffect, useState } from 'react';
import type { CommerceApi, ProductDto } from '@nexa/contracts';
export function ProductList({ products, onSelect }: {
  products: ProductDto[];
  onSelect?: (productId: string) => void;
}) {
  if (products.length === 0) return <p>No products available.</p>;

  return <ul style={{ listStyle: 'none', padding: 0, display: 'grid', gap: 12 }}>
    {products.map(p => <li key={p.id} style={{ border: '1px solid #cbd5e1', borderRadius: 12, padding: 16 }}>
      {onSelect
        ? <button type="button" onClick={() => onSelect(p.id)}>{p.title}</button>
        : <strong>{p.title}</strong>}
      <div>SKU: {p.sku}</div>
    </li>)}
  </ul>;
}

export function Catalog({ api, title }: { api: CommerceApi; title: string }) {
  const [products, setProducts] = useState<ProductDto[] | null>(null);
  const [error, setError] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    api.listProducts().then(
      (result) => { if (current) setProducts(result); },
      (err) => { if (current) setError(String(err)); },
    );
    return () => { current = false; };
  }, [api]);

  return <main style={{ fontFamily: 'system-ui', maxWidth: 700, margin: '48px auto', padding: 16 }}>
    <h1>{title}</h1>
    {selectedId ? <>
      <button type="button" onClick={() => setSelectedId(null)}>Back to products</button>
      <ProductDetail key={selectedId} api={api} productId={selectedId} />
    </> : <>
      {error && <p role="alert">Could not load products. {error}</p>}
      {!error && products === null && <p role="status">Loading products…</p>}
      {products && <ProductList products={products} onSelect={setSelectedId} />}
    </>}
  </main>;
}

function ProductDetail({ api, productId }: { api: CommerceApi; productId: string }) {
  const [product, setProduct] = useState<ProductDto | null | undefined>(undefined);
  const [error, setError] = useState('');

  useEffect(() => {
    let current = true;
    api.getProduct(productId).then(
      (result) => { if (current) setProduct(result); },
      (err) => { if (current) setError(String(err)); },
    );
    return () => { current = false; };
  }, [api, productId]);

  if (error) return <p role="alert">Could not load product details. {error}</p>;
  if (product === undefined) return <p role="status">Loading product details…</p>;
  if (product === null) return <p role="status">This product is no longer available.</p>;

  return <section aria-labelledby="product-title">
    <h2 id="product-title">{product.title}</h2>
    <dl>
      <dt>SKU</dt>
      <dd>{product.sku}</dd>
      <dt>Status</dt>
      <dd>{product.status}</dd>
    </dl>
  </section>;
}
