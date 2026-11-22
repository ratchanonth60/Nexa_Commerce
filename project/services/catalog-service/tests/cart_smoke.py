"""Verify the Cart HTTP API with an isolated test issuer and seeded databases."""

from concurrent.futures import ThreadPoolExecutor
import base64
import json
import os
import subprocess
import sys
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from uuid import uuid4


def encode(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def issue(subject, token_type="at+jwt", **overrides):
    header = {"alg": "RS256", "typ": token_type}
    claims = {
        "iss": os.environ["OIDC_ISSUER"],
        "aud": os.environ["OIDC_API_AUDIENCE"],
        "sub": subject,
        "exp": int(time.time()) + 300,
    }
    claims.update(overrides)
    signing_input = b".".join(encode(json.dumps(part).encode()) for part in (header, claims))
    signature = subprocess.check_output(
        ["openssl", "dgst", "-sha256", "-sign", sys.argv[1]],
        input=signing_input,
    )
    return b".".join([signing_input, encode(signature)]).decode()


def request(path, expected, token=None, method="GET", data=None):
    headers = {"Content-Type": "application/json"} if data is not None else {}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = Request(
        "http://127.0.0.1:8082/api/v1/carts" + path,
        method=method,
        data=json.dumps(data).encode() if data is not None else None,
        headers=headers,
    )
    try:
        with urlopen(req, timeout=10) as res:
            assert res.status == expected, (method, path, res.status, expected)
            return json.load(res)
    except HTTPError as err:
        assert err.code == expected, (method, path, err.code, expected)


def main():
    subject = str(uuid4())
    owner = issue(subject)
    stranger = issue(str(uuid4()))
    request("", 401, method="POST")
    request("", 401, "invalid-token", method="POST")
    request("", 401, issue(subject, aud="another-api"), method="POST")
    request("", 401, issue(subject, iss="https://wrong.example"), method="POST")
    request("", 401, issue(subject, exp=1), method="POST")
    request("", 401, issue(subject, token_type="JWT"), method="POST")

    cart = request("", 201, owner, "POST")
    path = "/" + cart["id"]
    assert cart == {"id": cart["id"], "items": [], "version": 0}
    assert request(path, 200, owner) == cart
    request(path, 403, stranger)
    product = "019b0000-0000-7000-8000-000000000001"
    item = {"product_id": product, "quantity": 2}
    request(path + "/items", 403, stranger, "POST", item)
    request(path + "/items", 404, owner, "POST", item | {"product_id": str(uuid4())})

    def add_item(_):
        return request(path + "/items", 200, owner, "POST", item)

    with ThreadPoolExecutor(max_workers=4) as pool:
        versions = sorted(result["version"] for result in pool.map(add_item, range(4)))
    assert versions == [1, 2, 3, 4], versions
    updated = request(path, 200, owner)
    assert updated == {"id": cart["id"], "version": 4, "items": [item | {"quantity": 8}]}
    request(path + "/items", 409, owner, "POST", item | {"quantity": 9999})
    assert request(path, 200, owner) == updated
    print("Cart authentication, ownership and concurrent updates passed")


if __name__ == "__main__":
    main()
