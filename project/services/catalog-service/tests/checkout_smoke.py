"""Verify sandbox checkout with the local gateway, test issuer, and fresh seeded databases.

Pass the issuer RSA private key as the first argument, as with cart_smoke.py.
"""

from concurrent.futures import ThreadPoolExecutor
import json
from threading import Barrier
import time
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen
from uuid import uuid4

from cart_smoke import issue, request as cart_request


MUG = "019b0000-0000-7000-8000-000000000002"
TSHIRT = "019b0000-0000-7000-8000-000000000001"
SEEDED_MUG_AVAILABILITY = 12
CHECKOUTS = "/checkouts"


def request(path, expected, token=None, *, data=None, key=None):
    headers = {}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    if data is not None:
        headers["Content-Type"] = "application/json"
    if key is not None:
        headers["Idempotency-Key"] = key

    method = "POST" if data is not None else "GET"
    req = Request(
        "http://127.0.0.1:8082/api/v1" + path,
        method=method,
        data=json.dumps(data).encode() if data is not None else None,
        headers=headers,
    )
    try:
        with urlopen(req, timeout=10) as response:
            assert response.status == expected, (method, path, response.status, expected)
            return json.load(response)
    except HTTPError as error:
        assert error.code == expected, (method, path, error.code, expected)


def add_item(token, cart_id, product_id, quantity):
    return cart_request(
        f"/{cart_id}/items",
        200,
        token,
        "POST",
        {"product_id": product_id, "quantity": quantity},
    )


def checkout_payload(token, product_id, quantity, payment):
    cart = cart_request("", 201, token, "POST")
    cart = add_item(token, cart["id"], product_id, quantity)
    return {
        "cart_id": cart["id"],
        "cart_version": cart["version"],
        "payment_method_token": payment,
    }


def poll_checkout(token, checkout_id):
    path = f"{CHECKOUTS}/{checkout_id}"
    deadline = time.monotonic() + 15
    while True:
        status = request(path, 200, token)
        assert status["id"] == checkout_id, status
        if status["status"] in {"confirmed", "failed"}:
            return status
        assert status["status"] == "created", status
        assert time.monotonic() < deadline, ("checkout did not reach a terminal state", status)
        time.sleep(0.1)


def check_availability(product_id, quantity, available):
    query = urlencode({"product_id": product_id, "quantity": quantity})
    result = request(f"/inventory/availability?{query}", 200)
    assert result == {"product_id": product_id, "quantity": quantity, "available": available}, result


def assert_failure(token, checkout_id, failure_reason):
    result = poll_checkout(token, checkout_id)
    assert result == {
        "id": checkout_id,
        "status": "failed",
        "order_id": None,
        "failure_reason_code": failure_reason,
    }, result


def main():
    owner = issue(str(uuid4()))
    stranger = issue(str(uuid4()))

    check_availability(MUG, SEEDED_MUG_AVAILABILITY, True)
    check_availability(TSHIRT, 1, False)

    decline = checkout_payload(owner, MUG, SEEDED_MUG_AVAILABILITY, "sandbox_decline")
    declined = request(CHECKOUTS, 202, owner, data=decline, key=str(uuid4()))
    assert_failure(owner, declined["id"], "payment_declined")
    check_availability(MUG, SEEDED_MUG_AVAILABILITY, True)

    shortage = checkout_payload(owner, TSHIRT, 1, "sandbox_approve")
    rejected = request(CHECKOUTS, 202, owner, data=shortage, key=str(uuid4()))
    assert_failure(owner, rejected["id"], "stock_unavailable")
    check_availability(TSHIRT, 1, False)

    quantity = 2
    approve = checkout_payload(owner, MUG, quantity, "sandbox_approve")
    key = str(uuid4())
    request(CHECKOUTS, 403, stranger, data=approve, key=key)
    callers = Barrier(4, timeout=10)

    def submit(_):
        callers.wait()
        return request(CHECKOUTS, 202, owner, data=approve, key=key)

    with ThreadPoolExecutor(max_workers=callers.parties) as pool:
        responses = list(pool.map(submit, range(callers.parties)))
    checkout_id = responses[0]["id"]
    assert all(response["id"] == checkout_id for response in responses), responses

    changed_payment = approve | {"payment_method_token": "sandbox_decline"}
    request(CHECKOUTS, 409, owner, data=changed_payment, key=key)
    request(f"{CHECKOUTS}/{checkout_id}", 403, stranger)

    confirmed = poll_checkout(owner, checkout_id)
    assert confirmed["status"] == "confirmed", confirmed
    assert confirmed["failure_reason_code"] is None, confirmed
    assert confirmed["order_id"], confirmed
    order_path = f"/orders/{confirmed['order_id']}"
    request(order_path, 403, stranger)
    order = request(order_path, 200, owner)
    assert order == {
        "id": confirmed["order_id"],
        "status": "confirmed",
        "currency": "THB",
        "total_minor": "49800",
        "items": [
            {
                "product_id": MUG,
                "sku": "DEMO-MUG",
                "title": "Nexa Demo Mug",
                "quantity": quantity,
                "unit_amount_minor": "24900",
                "rule_version": 1,
            }
        ],
    }, order
    check_availability(MUG, SEEDED_MUG_AVAILABILITY - quantity, True)
    check_availability(MUG, SEEDED_MUG_AVAILABILITY - quantity + 1, False)

    add_item(owner, approve["cart_id"], MUG, 1)
    request(CHECKOUTS, 409, owner, data=approve, key=str(uuid4()))
    replay = request(CHECKOUTS, 202, owner, data=approve, key=key)
    assert replay == confirmed, replay
    assert request(order_path, 200, owner) == order
    print("Checkout replay, ownership, order snapshot, and sandbox failure compensation passed")


if __name__ == "__main__":
    main()
