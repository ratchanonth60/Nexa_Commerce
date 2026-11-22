use axum::http::StatusCode;
use diesel::{
    sql_query,
    sql_types::{BigInt, Binary, Bool, Text},
    QueryableByName,
};
use diesel_async::{
    pooled_connection::{
        bb8::{Pool, PooledConnection},
        AsyncDieselConnectionManager, ManagerConfig,
    },
    AnsiTransactionManager, AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl,
    SimpleAsyncConnection, TransactionManager,
};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use sha2::{Digest, Sha384};

pub(crate) type PgPool = Pool<AsyncPgConnection>;
type StartupError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Query(#[from] diesel::result::Error),
    #[error("could not acquire PostgreSQL connection: {0}")]
    Pool(#[from] diesel_async::pooled_connection::bb8::RunError),
    #[error("{0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Status(StatusCode),
}

impl From<StatusCode> for Error {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl Error {
    pub(crate) fn into_status(self, fallback: impl FnOnce(Self) -> StatusCode) -> StatusCode {
        match self {
            Self::Status(status) => status,
            error => fallback(error),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DbMigrations {
    Catalog,
    Pricing,
    Inventory,
    Cart,
    Order,
    Payment,
    Fulfillment,
}

macro_rules! migrations {
    ($directory:literal, $($migration:literal),+ $(,)?) => {
        {
            const LEGACY: &[(&str, &[u8])] = &[$(($migration, include_bytes!(concat!("../", $directory, "/", $migration, "/up.sql")))),+];
            const EMBEDDED: EmbeddedMigrations = embed_migrations!($directory);
            (EMBEDDED, LEGACY)
        }
    };
}

impl DbMigrations {
    fn sources(self) -> (EmbeddedMigrations, &'static [(&'static str, &'static [u8])]) {
        match self {
            Self::Catalog => migrations!(
                "migrations",
                "202611220001_init",
                "202611220002_product_search"
            ),
            Self::Pricing => migrations!("pricing-migrations", "202611220001_init"),
            Self::Inventory => migrations!(
                "inventory-migrations",
                "202611220001_init",
                "202611220002_checkout_reservations",
                "202611220003_inventory_operations"
            ),
            Self::Cart => migrations!("cart-migrations", "202611220001_init"),
            Self::Order => migrations!(
                "order-migrations",
                "202611220001_checkout",
                "202611220002_fulfillment_delivery"
            ),
            Self::Payment => migrations!("payment-migrations", "202611220001_init"),
            Self::Fulfillment => migrations!(
                "fulfillment-migrations",
                "202611220001_init",
                "202611220002_dispatch_queue"
            ),
        }
    }
}

pub(crate) async fn connect(url: &str, migrations: DbMigrations) -> Result<PgPool, StartupError> {
    let pool = PgPool::builder().build(manager(url)?).await?;
    migrate(&pool, migrations).await?;
    Ok(pool)
}

pub(crate) async fn connection(
    pool: &PgPool,
) -> Result<PooledConnection<'_, AsyncPgConnection>, Error> {
    Ok(pool.get().await?)
}

fn manager(url: &str) -> Result<AsyncDieselConnectionManager<AsyncPgConnection>, StartupError> {
    let (config, roots_file) = postgres_config(url)?;
    let tls = postgres_tls(roots_file.as_deref())?;
    let mut settings = ManagerConfig::default();
    settings.custom_setup = Box::new(move |_| {
        let config = config.clone();
        let tls = tls.clone();
        Box::pin(async move {
            let (client, connection) = config
                .connect(tls)
                .await
                .map_err(|err| diesel::ConnectionError::BadConnection(err.to_string()))?;
            AsyncPgConnection::try_from_client_and_connection(client, connection).await
        })
    });
    Ok(AsyncDieselConnectionManager::new_with_config(url, settings))
}

fn postgres_config(url: &str) -> Result<(tokio_postgres::Config, Option<String>), StartupError> {
    // tokio-postgres uses certificate and hostname verification for TLS, and
    // represents both verified modes as a required TLS connection.
    let mut parsed = url::Url::parse(url)?;
    let mut roots_file = None;
    let pairs: Vec<_> = parsed.query_pairs().into_owned().collect();
    if parsed.query().is_some() {
        parsed.set_query(None);
        for (key, value) in pairs {
            match (key.as_str(), value.as_str()) {
                ("sslrootcert", _) => roots_file = Some(value),
                ("sslmode", "verify-ca" | "verify-full") => {
                    parsed.query_pairs_mut().append_pair(&key, "require");
                }
                _ => {
                    parsed.query_pairs_mut().append_pair(&key, &value);
                }
            }
        }
    }
    Ok((parsed.as_str().parse()?, roots_file))
}

fn postgres_tls(
    roots_file: Option<&str>,
) -> Result<tokio_postgres_rustls::MakeRustlsConnect, StartupError> {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(path) = roots_file {
        let pem = std::fs::read(path)?;
        use rustls::pki_types::pem::PemObject;
        for cert in rustls::pki_types::CertificateDer::pem_slice_iter(&pem) {
            roots.add(cert?)?;
        }
        if roots.is_empty() {
            return Err("sslrootcert contains no certificates".into());
        }
    } else {
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    }
    let tls = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(tls))
}

#[derive(QueryableByName)]
struct DatabaseName {
    #[diesel(sql_type = Text)]
    database_name: String,
}

#[derive(QueryableByName)]
struct LegacyMigration {
    #[diesel(sql_type = BigInt)]
    version: i64,
    #[diesel(sql_type = Bool)]
    success: bool,
    #[diesel(sql_type = Binary)]
    checksum: Vec<u8>,
}

#[derive(QueryableByName)]
struct LegacyHistory {
    #[diesel(sql_type = Bool)]
    present: bool,
}

async fn migrate(pool: &PgPool, migrations: DbMigrations) -> Result<(), StartupError> {
    let mut conn = pool.get_owned().await?;
    // The harness owns the connection. An error before commit drops the
    // open transaction, which diesel-async marks as broken for the pool.
    AnsiTransactionManager::begin_transaction(&mut *conn).await?;
    let database = sql_query("SELECT current_database()::text AS database_name")
        .get_result::<DatabaseName>(&mut conn)
        .await?;
    // Use SQLx's lock ID so older and newer startup code serialize on this database.
    let lock = 0x3d32ad9e_i64 * i64::from(crc32fast::hash(database.database_name.as_bytes()));
    sql_query("SELECT pg_advisory_xact_lock($1)")
        .bind::<BigInt, _>(lock)
        .execute(&mut conn)
        .await?;

    let (embedded, legacy_sources) = migrations.sources();
    let legacy = sql_query("SELECT to_regclass('_sqlx_migrations') IS NOT NULL AS present")
        .get_result::<LegacyHistory>(&mut conn)
        .await?;
    if legacy.present {
        adopt_legacy_history(&mut conn, legacy_sources).await?;
    }
    let mut conn = tokio::task::spawn_blocking(move || {
        let mut harness = AsyncMigrationHarness::new(conn);
        harness.run_pending_migrations(embedded)?;
        Ok::<_, StartupError>(harness.into_inner())
    })
    .await??;
    AnsiTransactionManager::commit_transaction(&mut *conn).await?;
    Ok(())
}

async fn adopt_legacy_history(
    conn: &mut AsyncPgConnection,
    sources: &[(&str, &[u8])],
) -> Result<(), StartupError> {
    let rows =
        sql_query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .load::<LegacyMigration>(conn)
            .await?;
    conn.batch_execute(
        "CREATE TABLE IF NOT EXISTS __diesel_schema_migrations (
            version VARCHAR(50) PRIMARY KEY NOT NULL,
            run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .await?;

    for (index, row) in rows.iter().enumerate() {
        let Some((version, sql)) = sources.get(index).and_then(|(name, sql)| {
            let (version, _) = name.split_once('_')?;
            (version.parse::<i64>().ok()? == row.version).then_some((version, sql))
        }) else {
            return Err(format!("unexpected SQLx migration version {}", row.version).into());
        };
        if !row.success || row.checksum.as_slice() != Sha384::digest(sql).as_slice() {
            return Err(format!(
                "SQLx migration {} failed or has a mismatched checksum",
                row.version
            )
            .into());
        }
        sql_query(
            "INSERT INTO __diesel_schema_migrations (version)
             VALUES ($1) ON CONFLICT (version) DO NOTHING",
        )
        .bind::<Text, _>(version)
        .execute(conn)
        .await?;
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn lazy_test_pool() -> PgPool {
    PgPool::builder().build_unchecked(AsyncDieselConnectionManager::new(
        "postgres://localhost/catalog_db",
    ))
}

#[cfg(test)]
pub(crate) async fn with_test_database<F, Fut>(migrations: DbMigrations, test: F)
where
    F: FnOnce(PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    with_isolated_database(migrations, async move |url| {
        test(connect(&url, migrations).await.unwrap()).await;
    })
    .await;
}

#[cfg(test)]
async fn with_isolated_database<F, Fut>(migrations: DbMigrations, test: F)
where
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let variable = match migrations {
        DbMigrations::Catalog => "DATABASE_URL",
        DbMigrations::Pricing => "PRICING_DATABASE_URL",
        DbMigrations::Inventory => "INVENTORY_DATABASE_URL",
        DbMigrations::Cart => "CART_DATABASE_URL",
        DbMigrations::Order => "ORDER_DATABASE_URL",
        DbMigrations::Payment => "PAYMENT_DATABASE_URL",
        DbMigrations::Fulfillment => "FULFILLMENT_DATABASE_URL",
    };
    let url = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable} must point to a test PostgreSQL server"));
    let admin_pool = PgPool::builder()
        .build(manager(&url).unwrap())
        .await
        .unwrap();
    let mut admin = connection(&admin_pool).await.unwrap();
    let name = format!("nexa_test_{}", uuid::Uuid::new_v4().simple());
    let mut test_url = url::Url::parse(&url).unwrap();
    test_url.set_path(&format!("/{name}"));
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let result = tokio::spawn(test(test_url.to_string())).await;
    admin
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error.into_panic());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(QueryableByName)]
    struct CatalogCheck {
        #[diesel(sql_type = Text)]
        title: String,
        #[diesel(sql_type = BigInt)]
        migration_count: i64,
        #[diesel(sql_type = Bool)]
        has_search_index: bool,
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn adopts_legacy_history_and_applies_only_pending_migrations() {
        with_isolated_database(DbMigrations::Catalog, async |url| {
            let pool = PgPool::builder()
                .build(manager(&url).unwrap())
                .await
                .unwrap();
            let mut conn = connection(&pool).await.unwrap();
            let legacy_sql = include_str!("../migrations/202611220001_init/up.sql");
            conn.batch_execute(legacy_sql).await.unwrap();
            conn.batch_execute(
                "UPDATE product SET title = 'Existing mug' WHERE sku = 'DEMO-MUG';
                 CREATE TABLE _sqlx_migrations (
                     version BIGINT PRIMARY KEY, success BOOLEAN NOT NULL, checksum BYTEA NOT NULL
                 );
                 INSERT INTO _sqlx_migrations VALUES (202611220001, TRUE, decode('00', 'hex'));",
            )
            .await
            .unwrap();
            drop(conn);

            let error = migrate(&pool, DbMigrations::Catalog).await.unwrap_err();
            assert!(error.to_string().contains("mismatched checksum"));

            let mut conn = connection(&pool).await.unwrap();
            let legacy = sql_query(
                "SELECT to_regclass('__diesel_schema_migrations') IS NOT NULL AS present",
            )
            .get_result::<LegacyHistory>(&mut conn)
            .await
            .unwrap();
            assert!(!legacy.present);
            sql_query("UPDATE _sqlx_migrations SET checksum = $1")
                .bind::<Binary, _>(Sha384::digest(legacy_sql).to_vec())
                .execute(&mut conn)
                .await
                .unwrap();
            drop(conn);

            // Concurrent startup must serialize before either process reads history.
            tokio::try_join!(
                migrate(&pool, DbMigrations::Catalog),
                migrate(&pool, DbMigrations::Catalog),
            )
            .unwrap();
            let check = sql_query(
                "SELECT title,
                    (SELECT count(*) FROM __diesel_schema_migrations) AS migration_count,
                    EXISTS (SELECT 1 FROM pg_indexes WHERE tablename = 'product'
                            AND indexdef LIKE '%USING gin%') AS has_search_index
                 FROM product WHERE sku = 'DEMO-MUG'",
            )
            .get_result::<CatalogCheck>(&mut connection(&pool).await.unwrap())
            .await
            .unwrap();
            assert_eq!(check.title, "Existing mug");
            assert_eq!(check.migration_count, 2);
            assert!(check.has_search_index);
        })
        .await;
    }
}
