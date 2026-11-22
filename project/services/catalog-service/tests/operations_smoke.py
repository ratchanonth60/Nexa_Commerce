"""Verify search, operator RBAC, and shipment delivery on an isolated gateway."""

import json
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from uuid import uuid4

from cart_smoke import issue
from checkout_smoke import MUG, checkout_payload, poll_checkout, request


def adjust_stock(path, token, before, after, expected):
    req = Request(
        "http://127.0.0.1:8082/api/v1" + path,
        method="PATCH",
        data=json.dumps({"expected_on_hand": before, "on_hand": after}).encode(),
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urlopen(req, timeout=10) as response:
            assert response.status == expected
            return json.load(response)
    except HTTPError as error:
        assert error.code == expected, (path, error.code, expected)


def wait_for_shipment(path, owner, status):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        req = Request("http://127.0.0.1:8082/api/v1" + path, headers={
            "Authorization": "Bearer " + owner,
        })
        try:
            with urlopen(req, timeout=10) as response:
                shipment = json.load(response)
                if shipment["status"] == status:
                    return shipment
        except HTTPError as error:
            assert error.code == 404
        time.sleep(0.15)
    raise AssertionError(f"Shipment did not reach {status}")


def main():
    owner = issue(str(uuid4()))
    other = issue(str(uuid4()))
    operator = issue(str(uuid4()), nexa_roles=["nexa-operator"])

    products = request("/search/products?q=mug", 200)
    assert [product["id"] for product in products] == [MUG], products
    assert request("/search/products?q=absentproduct", 200) == []
    request("/search/products?q=%20", 400)

    stock_path = f"/ops/inventory/{MUG}"
    request(stock_path, 401)
    request(stock_path, 403, owner)
    before = request(stock_path, 200, operator)
    adjusted = adjust_stock(stock_path, operator, before["on_hand"], before["on_hand"] + 1, 200)
    assert adjusted["on_hand"] == before["on_hand"] + 1
    adjust_stock(stock_path, operator, before["on_hand"], before["on_hand"], 409)

    checkout = request("/checkouts", 202, owner,
                       data=checkout_payload(owner, MUG, 1, "sandbox_approve"), key=str(uuid4()))
    confirmed = poll_checkout(owner, checkout["id"])
    assert confirmed["status"] == "confirmed", confirmed
    order_id = confirmed["order_id"]
    customer_path = f"/orders/{order_id}/shipment"
    operator_path = f"/ops/shipments/{order_id}"
    notifications = f"/orders/{order_id}/notifications"
    initial = wait_for_shipment(customer_path, owner, "pending")
    request(customer_path, 403, other)
    request(notifications, 403, other)
    assert request(notifications, 200, owner) == []

    pack = {"expected_status": "pending", "status": "packed"}
    request(operator_path, 403, owner, data=pack)
    request(operator_path, 400, operator, data={**pack, "status": "delivered"})
    assert request(operator_path, 200, operator, data=pack)["status"] == "packed"
    request(operator_path, 202, operator, data={"expected_status": "packed", "status": "dispatched"})
    dispatched = wait_for_shipment(customer_path, owner, "dispatched")
    assert dispatched["id"] == initial["id"]
    after = request(stock_path, 200, operator)
    assert after["on_hand"] == before["on_hand"] and after["allocated"] == before["allocated"]
    request(operator_path, 200, operator, data={"expected_status": "packed", "status": "dispatched"})
    request(operator_path, 200, operator, data={"expected_status": "dispatched", "status": "delivered"})
    assert wait_for_shipment(customer_path, owner, "delivered")["id"] == initial["id"]
    events = request(notifications, 200, owner)
    assert [event["status"] for event in events] == ["dispatched", "delivered"], events

    print("Search, stock RBAC, durable dispatch, shipment ownership, and notifications passed")


if __name__ == "__main__":
    main()
