import { useEffect, useState } from 'react';
import { CheckoutPanel } from './CheckoutPanel';
import type { Cart, CommerceApi, CustomerApi, PriceQuote, ProductDto } from '@nexa/contracts';
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

export function Catalog({ api, customerApi, title }: {
  api: CommerceApi;
  customerApi?: CustomerApi;
  title: string;
}) {
  const [products, setProducts] = useState<ProductDto[] | null>(null);
  const [error, setError] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [searchInput, setSearchInput] = useState('');
  const [searchQuery, setSearchQuery] = useState('');
  const [signedIn, setSignedIn] = useState(false);
  const [cart, setCart] = useState<Cart | null>(null);
  const [checkoutPending, setCheckoutPending] = useState(false);
  const [busy, setBusy] = useState(false);
  const [accountError, setAccountError] = useState('');

  useEffect(() => {
    let current = true;
    customerApi?.signedIn().then(
      (result) => { if (current) setSignedIn(result); },
      () => { if (current) setAccountError('Could not check your session.'); },
    );
    return () => { current = false; };
  }, [customerApi]);

  async function accountAction(action: () => Promise<void>) {
    setBusy(true);
    setAccountError('');
    try {
      await action();
    } catch (err) {
      setAccountError(String(err));
      if (customerApi && await customerApi.signedIn().catch(() => true) === false) {
        setSignedIn(false);
        setCart(null);
        setCheckoutPending(false);
      }
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    let current = true;
    setProducts(null);
    setError('');
    const request = searchQuery && api.searchProducts
      ? api.searchProducts(searchQuery)
      : api.listProducts();
    request.then(
      (result) => { if (current) setProducts(result); },
      (err) => { if (current) setError(String(err)); },
    );
    return () => { current = false; };
  }, [api, searchQuery]);

  return <main style={{ fontFamily: 'system-ui', maxWidth: 700, margin: '48px auto', padding: 16 }}>
    <h1>{title}</h1>
    {customerApi && <section aria-label="Account and cart">
      <button type="button" disabled={busy} onClick={() => void accountAction(async () => {
        if (signedIn) {
          await customerApi.signOut();
          setSignedIn(false);
          setCart(null);
          setCheckoutPending(false);
        } else {
          await customerApi.signIn();
          setSignedIn(await customerApi.signedIn());
        }
      })}>{signedIn ? 'Sign out' : 'Sign in'}</button>
      {signedIn && <p role="status">Cart items: {cart?.items.reduce((total, item) => total + item.quantity, 0) ?? 0}</p>}
      {accountError && <p role="alert">{accountError}</p>}
    </section>}
    {customerApi?.checkout && signedIn && <CheckoutPanel api={customerApi.checkout}
      cart={cart} cartBusy={busy} onCheckoutState={(pending, confirmedCartId) => {
        setCheckoutPending(pending);
        if (confirmedCartId) setCart(current => current?.id === confirmedCartId ? null : current);
      }} />}
    {selectedId ? <>
      <button type="button" onClick={() => setSelectedId(null)}>Back to products</button>
      <ProductDetail key={selectedId} api={api} productId={selectedId} />
      {customerApi && signedIn && <button type="button" disabled={busy || checkoutPending}
        onClick={() => void accountAction(async () => {
          const currentCart = cart ?? await customerApi.createCart();
          setCart(currentCart);
          setCart(await customerApi.addCartItem(currentCart.id, selectedId, 1));
        })}>Add to cart</button>}
    </> : <>
      {api.searchProducts && <form onSubmit={(event) => {
        event.preventDefault();
        setSearchQuery(searchInput.trim());
      }}>
        <label htmlFor="catalog-search">Search products </label>
        <input id="catalog-search" type="search" maxLength={120}
          value={searchInput} onChange={(event) => setSearchInput(event.target.value)} />
        <button type="submit">Search</button>
      </form>}
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
    <ProductPrice api={api} productId={product.id} />
    <ProductAvailability api={api} productId={product.id} />
  </section>;
}

function ProductPrice({ api, productId }: { api: CommerceApi; productId: string }) {
  const [quote, setQuote] = useState<PriceQuote | null | undefined>(undefined);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let current = true;
    api.getPriceQuote(productId, 'THB', 'storefront').then(
      (result) => { if (current) setQuote(result); },
      () => { if (current) setFailed(true); },
    );
    return () => { current = false; };
  }, [api, productId]);

  if (failed) return <p role="alert">Could not load the price.</p>;
  if (quote === undefined) return <p role="status">Loading price…</p>;
  if (quote === null) return <p role="status">Price unavailable.</p>;

  const amount = quote.unit_amount_minor.padStart(3, '0');
  return <p>{quote.currency} {amount.slice(0, -2)}.{amount.slice(-2)}</p>;
}

function ProductAvailability({ api, productId }: { api: CommerceApi; productId: string }) {
  const [available, setAvailable] = useState<boolean | null | undefined>(undefined);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let current = true;
    api.getAvailability(productId, 1).then(
      (result) => { if (current) setAvailable(result?.available ?? null); },
      () => { if (current) setFailed(true); },
    );
    return () => { current = false; };
  }, [api, productId]);

  if (failed) return <p role="alert">Could not check stock.</p>;
  if (available === undefined) return <p role="status">Checking stock…</p>;
  if (available === null) return <p role="status">Stock information unavailable.</p>;
  return <p>{available ? 'In stock' : 'Out of stock'}</p>;
}
