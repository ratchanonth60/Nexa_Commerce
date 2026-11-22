CREATE TYPE sandbox_payment_status AS ENUM ('authorized', 'declined', 'voided');

CREATE TABLE payment_authorization (
    checkout_id uuid PRIMARY KEY,
    amount_minor bigint CHECK (amount_minor >= 0),
    sandbox_approved boolean,
    status sandbox_payment_status NOT NULL,
    CHECK ((amount_minor IS NULL) = (sandbox_approved IS NULL)),
    CHECK (
        status = 'voided'
        OR (amount_minor IS NOT NULL AND sandbox_approved = (status = 'authorized'))
    )
);
