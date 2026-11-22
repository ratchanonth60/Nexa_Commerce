import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import './backoffice.css';

type Stock = {
  product_id: string;
  on_hand: number;
  reserved: number;
  allocated: number;
};

type ShipmentStatus = 'pending' | 'packed' | 'dispatched' | 'delivered';

type Shipment = {
  id: string;
  order_id: string;
  status: ShipmentStatus;
};

type ShipmentAdvance = { shipment: Shipment; queued: boolean };

const statusLabels: Record<ShipmentStatus, string> = {
  pending: 'Pending',
  packed: 'Packed',
  dispatched: 'Dispatched',
  delivered: 'Delivered',
};

const shipmentActions: Partial<Record<ShipmentStatus, { status: ShipmentStatus; label: string }>> = {
  pending: { status: 'packed', label: 'Mark packed' },
  packed: { status: 'dispatched', label: 'Dispatch shipment' },
  dispatched: { status: 'delivered', label: 'Mark delivered' },
};

export function Backoffice() {
  return <aside className="backoffice" aria-labelledby="backoffice-heading">
    <h2 id="backoffice-heading">Backoffice</h2>
    <div className="backoffice-panels">
      <Inventory />
      <Shipments />
    </div>
  </aside>;
}

function Inventory() {
  const [productId, setProductId] = useState('');
  const [stock, setStock] = useState<Stock | null>(null);
  const [quantity, setQuantity] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');

  async function update(action: () => Promise<Stock>, saved: boolean) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      const current = await action();
      setStock(current);
      setQuantity(String(current.on_hand));
      if (saved) setNotice('Stock updated.');
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return <section className="backoffice-panel" aria-labelledby="inventory-heading">
    <h3 id="inventory-heading">Inventory</h3>
    <form onSubmit={(event) => {
      event.preventDefault();
      setStock(null);
      void update(() => invoke('read_stock', { productId: productId.trim() }), false);
    }}>
      <label>Product UUID
        <input value={productId} required disabled={busy} spellCheck={false} onChange={(event) => {
          setProductId(event.target.value);
          setStock(null);
          setError('');
          setNotice('');
        }} />
      </label>
      <button type="submit" disabled={busy}>Load stock</button>
    </form>
    {stock && <>
      <dl>
        <div><dt>On hand</dt><dd>{stock.on_hand}</dd></div>
        <div><dt>Reserved</dt><dd>{stock.reserved}</dd></div>
        <div><dt>Allocated</dt><dd>{stock.allocated}</dd></div>
      </dl>
      <form onSubmit={(event) => {
        event.preventDefault();
        void update(() => invoke('adjust_stock', {
          productId: stock.product_id,
          expectedOnHand: stock.on_hand,
          onHand: Number(quantity),
        }), true);
      }}>
        <label>New on-hand quantity
          <input type="number" min={stock.reserved + stock.allocated} max={2147483647}
            step={1} value={quantity} required disabled={busy}
            onChange={(event) => setQuantity(event.target.value)} />
        </label>
        <button type="submit" disabled={busy || !quantity || Number(quantity) === stock.on_hand}>
          Save stock
        </button>
      </form>
    </>}
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {busy && <p role="status">Working…</p>}
  </section>;
}

function Shipments() {
  const [orderId, setOrderId] = useState('');
  const [shipment, setShipment] = useState<Shipment | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [queuedOrder, setQueuedOrder] = useState<string | null>(null);
  const poll = useRef<AbortController | null>(null);

  useEffect(() => () => poll.current?.abort(), []);

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

  async function load() {
    const current = await invoke<Shipment>('read_shipment', { orderId: orderId.trim() });
    setShipment(current);
    if (current.status !== 'packed') {
      setQueuedOrder((queued) => queued === current.order_id ? null : queued);
    }
  }

  async function advance(current: Shipment, status: ShipmentStatus) {
    const controller = new AbortController();
    poll.current = controller;
    const result = await invoke<ShipmentAdvance>('advance_shipment', {
      orderId: current.order_id,
      expectedStatus: current.status,
      status,
    });
    if (controller.signal.aborted) return;
    setShipment(result.shipment);
    if (!result.queued) return;

    setQueuedOrder(current.order_id);
    for (let attempt = 0; attempt < 8; attempt++) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, 500));
      if (controller.signal.aborted) return;
      const latest = await invoke<Shipment>('read_shipment', { orderId: current.order_id });
      if (controller.signal.aborted) return;
      if (latest.status !== 'packed') {
        setShipment(latest);
        setQueuedOrder(null);
        return;
      }
    }
  }

  const next = shipment && shipmentActions[shipment.status];
  const queued = shipment?.order_id === queuedOrder;
  return <section className="backoffice-panel" aria-labelledby="shipments-heading">
    <h3 id="shipments-heading">Shipments</h3>
    <form onSubmit={(event) => {
      event.preventDefault();
      setShipment(null);
      void update(load);
    }}>
      <label>Order UUID
        <input value={orderId} required disabled={busy} spellCheck={false} onChange={(event) => {
          setOrderId(event.target.value);
          setShipment(null);
          setError('');
        }} />
      </label>
      <button type="submit" disabled={busy}>Load shipment</button>
    </form>
    {shipment && <>
      <dl>
        <div><dt>Shipment ID</dt><dd>{shipment.id}</dd></div>
        <div><dt>Status</dt><dd role="status">{statusLabels[shipment.status]}</dd></div>
      </dl>
      {next && !queued && <button type="button" disabled={busy}
        onClick={() => void update(() => advance(shipment, next.status))}>{next.label}</button>}
    </>}
    {error && <p role="alert">{error}</p>}
    {queued && <p role="status">{busy
      ? 'Dispatch queued. Checking shipment status…'
      : 'Dispatch is still queued. Load shipment to check again.'}</p>}
    {busy && !queued && <p role="status">Working…</p>}
  </section>;
}
