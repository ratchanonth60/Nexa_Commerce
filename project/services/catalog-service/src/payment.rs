use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use crate::db::{connection, Error, PgPool};

#[derive(QueryableByName)]
struct PaymentDecision {
    #[diesel(sql_type = Bool)]
    matches: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxOutcome {
    Approve,
    Decline,
}

pub(crate) async fn authorize(
    pool: &PgPool,
    checkout_id: Uuid,
    amount_minor: i64,
    outcome: SandboxOutcome,
) -> Result<bool, Error> {
    if amount_minor < 0 {
        return Err(Error::InvalidArgument(
            "payment amount must be nonnegative".into(),
        ));
    }

    // Preserve the original invoice across retries. An authorization that arrives
    // after a void can fill in the invoice but cannot change the terminal status.
    sql_query(
        "INSERT INTO payment_authorization AS existing
            (checkout_id, amount_minor, sandbox_approved, status)
         VALUES ($1, $2, $3,
                 (CASE WHEN $3 THEN 'authorized' ELSE 'declined' END)::sandbox_payment_status)
         ON CONFLICT (checkout_id) DO UPDATE
            SET amount_minor = COALESCE(existing.amount_minor, EXCLUDED.amount_minor),
                sandbox_approved = COALESCE(existing.sandbox_approved, EXCLUDED.sandbox_approved)
         WHERE existing.amount_minor IS NULL
            OR (existing.amount_minor = EXCLUDED.amount_minor
                AND existing.sandbox_approved = EXCLUDED.sandbox_approved)
         RETURNING (existing.status = 'authorized') AS matches",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .bind::<BigInt, _>(amount_minor)
    .bind::<Bool, _>(outcome == SandboxOutcome::Approve)
    .get_result::<PaymentDecision>(&mut *connection(pool).await?)
    .await
    .optional()?
    .map(|row| row.matches)
    .ok_or_else(|| Error::InvalidArgument("payment authorization invoice does not match".into()))
}

pub(crate) async fn void(pool: &PgPool, checkout_id: Uuid) -> Result<(), Error> {
    // A void with no authorization still prevents a delayed retry from authorizing.
    sql_query(
        "INSERT INTO payment_authorization AS existing (checkout_id, status)
         VALUES ($1, 'voided')
         ON CONFLICT (checkout_id) DO UPDATE SET status = EXCLUDED.status
         WHERE existing.status = 'authorized'",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .execute(&mut *connection(pool).await?)
    .await?;
    Ok(())
}

pub(crate) async fn was_declined(pool: &PgPool, checkout_id: Uuid) -> Result<bool, Error> {
    sql_query(
        "SELECT EXISTS (
            SELECT 1 FROM payment_authorization WHERE checkout_id = $1 AND status = 'declined'
         ) AS matches",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .get_result::<PaymentDecision>(&mut *connection(pool).await?)
    .await
    .map(|row| row.matches)
    .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::{authorize, void, Error, SandboxOutcome::*, Uuid};

    use crate::db::{with_test_database, DbMigrations};

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn authorization_reuses_the_matching_invoice_and_only_approved_payments_succeed() {
        with_test_database(DbMigrations::Payment, async |pool| {
            let approved = Uuid::from_u128(1);
            let (first, retry) = tokio::join!(
                authorize(&pool, approved, 0, Approve),
                authorize(&pool, approved, 0, Approve),
            );
            assert!(first.unwrap());
            assert!(retry.unwrap());
            for (amount, outcome) in [(1, Approve), (0, Decline)] {
                assert!(matches!(
                    authorize(&pool, approved, amount, outcome).await,
                    Err(Error::InvalidArgument(_))
                ));
            }

            let declined = Uuid::from_u128(2);
            assert!(!authorize(&pool, declined, 350, Decline).await.unwrap());
            assert!(!authorize(&pool, declined, 350, Decline).await.unwrap());
            void(&pool, declined).await.unwrap();
            assert!(!authorize(&pool, declined, 350, Decline).await.unwrap());

            void(&pool, approved).await.unwrap();
            void(&pool, approved).await.unwrap();
            assert!(!authorize(&pool, approved, 0, Approve).await.unwrap());
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn void_before_authorization_and_concurrent_compensation_are_terminal() {
        with_test_database(DbMigrations::Payment, async |pool| {
            let voided = Uuid::from_u128(1);
            void(&pool, voided).await.unwrap();
            void(&pool, voided).await.unwrap();
            assert!(!authorize(&pool, voided, 350, Approve).await.unwrap());
            assert!(matches!(
                authorize(&pool, voided, 351, Approve).await,
                Err(Error::InvalidArgument(_))
            ));

            let concurrent = Uuid::from_u128(2);
            let (payment, compensation) = tokio::join!(
                authorize(&pool, concurrent, 350, Approve),
                void(&pool, concurrent),
            );
            payment.unwrap();
            compensation.unwrap();
            assert!(!authorize(&pool, concurrent, 350, Approve).await.unwrap());
        })
        .await;
    }
}
