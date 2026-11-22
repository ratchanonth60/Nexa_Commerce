#!/usr/bin/env bash
set -euo pipefail
umask 077

fail() {
    printf 'Backup drill failed: %s\n' "$*" >&2
    exit 1
}

infra_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
project_dir=$(dirname -- "$infra_dir")
compose() {
    docker compose --project-directory "$project_dir" -f "$infra_dir/compose.yaml" \
        --profile checkout --profile operations "$@"
}

command -v docker >/dev/null || fail "Docker is required"
docker info --format '{{.ServerVersion}}' >/dev/null || fail "Docker daemon is unavailable"
compose version >/dev/null || fail "Docker Compose is required"

sources=(
    'postgres:catalog_db'
    'pricing-postgres:pricing_db'
    'inventory-postgres:inventory_db'
    'cart-postgres:cart_db'
    'order-postgres:order_db'
    'payment-postgres:payment_db'
    'fulfillment-postgres:fulfillment_db'
)
running=$(compose ps --status running --services)
for source in "${sources[@]}"; do
    service=${source%%:*}
    grep -Fxq -- "$service" <<< "$running" || fail "start the $service Compose service"
done

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/nexa-backup-drill.XXXXXXXX")
restore_container=''
cleanup() {
    local result=$?
    if [[ -n $restore_container ]] && ! docker rm --force --volumes "$restore_container" >/dev/null; then
        printf 'Could not remove temporary restore container %s\n' "$restore_container" >&2
        result=1
    fi
    if ! rm -rf -- "$work_dir"; then
        printf 'Could not remove private temporary backup directory %s\n' "$work_dir" >&2
        result=1
    fi
    return "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

suffix=${work_dir##*.}
restore_container=$(docker create --pull=never --network none \
    --name "nexa-backup-drill-$suffix" --env POSTGRES_USER=backup_drill \
    --env POSTGRES_HOST_AUTH_METHOD=trust postgres:17-alpine)
docker start "$restore_container" >/dev/null

deadline=$((SECONDS + 60))
# TCP readiness excludes the image entrypoint's temporary socket-only server.
until docker exec "$restore_container" pg_isready -q -h 127.0.0.1 -U backup_drill; do
    (( SECONDS < deadline )) || fail "isolated PostgreSQL server did not become ready"
    sleep 1
done

manifest_sql() {
    local database=$1 table
    cat <<'SQL'
SET LOCAL TIME ZONE 'UTC';
SET LOCAL DateStyle TO 'ISO, YMD';
SELECT format('SELECT %L, count(*) FROM %I.%I;',
              E'table\t' || format('%I.%I', schemaname, tablename), schemaname, tablename)
FROM pg_catalog.pg_tables
WHERE left(schemaname, 3) <> 'pg_' AND schemaname <> 'information_schema'
ORDER BY schemaname COLLATE "C", tablename COLLATE "C"
\gexec
SQL
    case "$database" in
        order_db)
            cat <<'SQL'
SELECT 'fixture', 'confirmed_orders', count(*) FROM public.checkout WHERE status = 'confirmed';
SELECT 'fixture', 'order_outbox', count(*) FROM public.order_outbox;
SQL
            set -- checkout checkout_item order_customer order_outbox
            ;;
        fulfillment_db)
            cat <<'SQL'
SELECT 'fixture', 'shipments', count(*) FROM public.shipment;
SELECT 'fixture', 'shipment_notifications', count(*) FROM public.fulfillment_notification_outbox;
SQL
            set -- shipment fulfillment_inbox fulfillment_notification_outbox
            ;;
        *) return 0 ;;
    esac

    # Fingerprint each critical row without printing customer data or payloads.
    for table in "$@"; do
        cat <<SQL
SELECT 'row', '$table', encode(sha256(convert_to(to_jsonb(row_data)::text, 'UTF8')), 'hex') COLLATE "C" AS fingerprint
FROM public.$table AS row_data ORDER BY fingerprint;
SQL
    done
}

# Run the dump while the manifest's read-only transaction holds the same exported snapshot.
source_export=$(cat <<'SH'
umask 077
export DRILL_DB="$1"
DRILL_WORK=$(mktemp -d /tmp/nexa-drill-source.XXXXXXXX)
export DRILL_WORK
trap 'rm -rf -- "$DRILL_WORK"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
{
    cat <<'SQL'
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT pg_export_snapshot() AS backup_snapshot \gset
\setenv DRILL_SNAPSHOT :backup_snapshot
\! pg_dump --no-password -U "$POSTGRES_USER" -d "$DRILL_DB" --format=custom --lock-wait-timeout=5s --snapshot="$DRILL_SNAPSHOT" --file="$DRILL_WORK/backup.dump"
\if :SHELL_ERROR
  \warn 'pg_dump failed'
  SELECT 1 / 0;
\endif
SQL
    cat
    printf 'COMMIT;\n'
} | psql -X -qAt -F "$(printf '\t')" --no-password -v ON_ERROR_STOP=1 \
    -U "$POSTGRES_USER" -d "$DRILL_DB" > "$DRILL_WORK/source.manifest"
tar -cf - -C "$DRILL_WORK" backup.dump source.manifest
SH
)

verify_source() {
    local database=$1 manifest=$2 table fixture
    case "$database" in
        catalog_db) set -- product ;;
        pricing_db) set -- price ;;
        inventory_db) set -- stock_record checkout_reservation checkout_reservation_item stock_adjustment ;;
        cart_db) set -- cart cart_item ;;
        order_db) set -- checkout checkout_item order_customer order_outbox ;;
        payment_db) set -- payment_authorization ;;
        fulfillment_db) set -- shipment fulfillment_inbox fulfillment_notification_outbox ;;
    esac
    for table in "$@"; do
        grep -Fq $'table\t'"public.$table"$'\t' "$manifest" || fail "$database is missing table $table"
    done

    case "$database" in
        order_db) set -- confirmed_orders order_outbox ;;
        fulfillment_db) set -- shipments shipment_notifications ;;
        *) return 0 ;;
    esac
    for fixture in "$@"; do
        grep -Eq $'^fixture\t'"$fixture"$'\t[1-9][0-9]*$' "$manifest" || \
            fail "$database needs at least one $fixture record for the drill"
    done
}

for source in "${sources[@]}"; do
    service=${source%%:*}
    database=${source#*:}
    db_work="$work_dir/$database"
    restored_database="drill_${database}_$(printf '%s' "$suffix" | tr '[:upper:]' '[:lower:]')"
    mkdir -- "$db_work"
    manifest_sql "$database" > "$db_work/verify.sql"

    if ! compose exec -T "$service" sh -eu -c "$source_export" sh "$database" \
        < "$db_work/verify.sql" 2> "$db_work/backup.log" | \
        tar -xf - -C "$db_work" backup.dump source.manifest; then
        fail "could not dump and inspect $database"
    fi
    verify_source "$database" "$db_work/source.manifest"

    docker exec "$restore_container" createdb --no-password -U backup_drill -T template0 "$restored_database" \
        2> "$db_work/restore.log" || fail "could not create isolated database for $database"
    docker exec -i "$restore_container" pg_restore --no-password --single-transaction --exit-on-error \
        --no-owner --no-privileges -U backup_drill -d "$restored_database" \
        < "$db_work/backup.dump" 2>> "$db_work/restore.log" || fail "could not restore $database"
    if ! { printf 'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;\n'; cat "$db_work/verify.sql"; printf 'COMMIT;\n'; } | \
        docker exec -i "$restore_container" psql -X -qAt -F $'\t' --no-password -v ON_ERROR_STOP=1 \
            -U backup_drill -d "$restored_database" > "$db_work/restored.manifest" 2>> "$db_work/restore.log"; then
        fail "could not inspect restored $database"
    fi
    cmp -s "$db_work/source.manifest" "$db_work/restored.manifest" || \
        fail "table, row-count, or critical-record comparison differs for $database"

    printf '%s verified\n' "$database"
    awk -F '\t' '$1 == "table" {printf "  %s %s rows\n", $2, $3}' "$db_work/restored.manifest"
done

printf 'Verified all seven PostgreSQL backups and restored records.\n'
