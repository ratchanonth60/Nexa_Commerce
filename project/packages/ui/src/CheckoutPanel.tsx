import { useRef, useState } from 'react';
import type { Cart, CheckoutApi, CheckoutStatus, Shipment, ShipmentNotification } from '@nexa/contracts';

export function CheckoutPanel({ api, cart, cartBusy, onCheckoutState }: {
  api: CheckoutApi;
  cart: Cart | null;
  cartBusy: boolean;
  onCheckoutState: (pending: boolean, confirmedCartId?: string) => void;
}) {
  const retry = useRef<{ cartId: string; version: number; key: string } | null>(null);
  const [checkout, setCheckout] = useState<CheckoutStatus | null>(null);
  const [shipment, setShipment] = useState<Shipment | null>(null);
  const [notifications, setNotifications] = useState<ShipmentNotification[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  async function update(action: () => Promise<void>) {
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function show(next: CheckoutStatus) {
    setCheckout(next);
    onCheckoutState(next.status === 'created', next.order_id ? retry.current?.cartId : undefined);
    if (next.status === 'failed') retry.current = null;
    if (next.order_id) {
      const [nextShipment, events] = await Promise.all([
        api.shipment(next.order_id), api.notifications(next.order_id),
      ]);
      setShipment(nextShipment);
      setNotifications(events);
    }
  }

  async function start(currentCart: Cart) {
    if (retry.current?.cartId !== currentCart.id || retry.current.version !== currentCart.version) {
      retry.current = { cartId: currentCart.id, version: currentCart.version, key: crypto.randomUUID() };
    }
    onCheckoutState(true);
    let next;
    try {
      next = await api.start(currentCart, retry.current.key);
    } catch (err) {
      onCheckoutState(false);
      throw err;
    }
    setShipment(null);
    setNotifications([]);
    await show(next);
  }

  if (!cart?.items.length && !checkout) return null;

  return <section aria-label="Sandbox checkout">
    {!!cart?.items.length && checkout?.status !== 'created' && (
      <button type="button" disabled={busy || cartBusy} onClick={() => void update(() => start(cart))}>
        Place sandbox order
      </button>
    )}
    {checkout && <>
      <p role="status">Checkout {checkout.status}{checkout.failure_reason_code && ` (${checkout.failure_reason_code})`}</p>
      {checkout.order_id && <p>Order ID <code>{checkout.order_id}</code></p>}
      {checkout.order_id && <p>Shipment {shipment?.status ?? 'awaiting creation'}</p>}
      {notifications.length > 0 && <ul aria-label="Shipment notifications">
        {notifications.map(event => <li key={event.event_id}>
          Shipment {event.status} · <time dateTime={event.occurred_at}>{new Date(event.occurred_at).toLocaleString()}</time>
        </li>)}
      </ul>}
      {checkout.status !== 'failed' && <button type="button" disabled={busy}
        onClick={() => void update(async () => { await show(await api.status(checkout.id)); })}>
        Refresh order status
      </button>}
    </>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
