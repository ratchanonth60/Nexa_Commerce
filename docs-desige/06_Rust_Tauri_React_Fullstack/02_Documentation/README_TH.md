# Nexa Commerce — Rust / Tauri / React Full-Stack Design

เอกสารส่วนนี้เป็น **Target Architecture** เพิ่มเติมจากชุด ERD และ Microservices เดิม โดยยังคงชื่อ **Nexa Commerce** และ 8 bounded contexts เดิม

## คำตอบการออกแบบหลัก

- **Rust backend**: Axum + Tokio + Diesel + PostgreSQL, 8 domain services ตาม ERD เดิม
- **Web app**: React + TypeScript + Vite สำหรับ Storefront / Admin UI
- **Desktop app**: Tauri 2 + Rust + React + TypeScript สำหรับ Backoffice / POS-style workstations (สมมติฐานที่ปรับได้)
- **Shared frontend**: Packages `ui`, `contracts`, `api-client`; ไม่ให้ Web app ใช้ `invoke()` โดยตรง
- **Gateway/BFF**: Rust Axum, HTTPS, JWT validation, authorization, API versioning, idempotency boundary
- **Async**: RabbitMQ + Transactional Outbox; Saga state ที่ Order & Checkout; idempotent event handlers
- **Data**: PostgreSQL แยก ownership ตาม service; Redis ใช้ cache ไม่ใช่ source of truth
- **Auth**: Web = OIDC with secure BFF cookie; Desktop = system-browser Authorization Code + PKCE; OS credential store
- **CI**: Rust unit/integration + SQL migrations + TS tests + contract compatibility + Tauri signing + SBOM

## ตำแหน่งไฟล์

| ไฟล์/โฟลเดอร์ | จุดประสงค์ |
|---|---|
| `01_Architecture/*.png` | รูป Architecture และแนวทาง Shared UI |
| `01_Architecture/*.svg` / `*.dot` / `*.mmd` | ต้นฉบับแก้ไขได้ |
| `01_Architecture/Checkout_Sequence.mmd` | Checkout Saga ข้ามบริการ |
| `02_Documentation/01_Architecture_Decisions.md` | บทบาทแต่ละชั้นและ trade-offs |
| `02_Documentation/02_Monorepo_Structure.md` | โครงสร้าง workspace Rust + pnpm |
| `02_Documentation/03_API_and_Event_Contracts.md` | API, Auth, Event, schema ownership |
| `02_Documentation/04_Tauri_Security_and_Desktop.md` | Capabilities / PKCE / updater / offline |
| `02_Documentation/05_Infrastructure_and_DevOps.md` | CI/CD, infra, observability |
| `02_Documentation/06_Implementation_Roadmap.md` | MVP + 4 phases |
| [`project/contracts/`](../../../project/contracts/) | สัญญา API และ Event JSON Schema ฉบับร่าง |
| [`project/`](../../../project/README.md) | Catalog demo ที่รันได้ พร้อม [OpenAPI](../../../project/contracts/nexa_gateway.openapi.yaml) |

## สิ่งที่ไม่ได้เปลี่ยนจากชุดเดิม

- Customer, Catalog, Pricing/Promotion, Inventory, Cart, Order/Checkout, Payment, Fulfillment
- Source of truth ในแต่ละ database ของ service
- No cross-database foreign keys; ข้าม service ใช้ UUID opaque reference
- Order price/address snapshots, inventory reservations, checkout saga + outbox

## ขอบเขตและข้อจำกัด

เอกสารนี้ไม่ใช่ production-ready application; ไม่ได้ implement payment gateway, tax rules, user authorization, OIDC login หรือ migrations ทุก service จริง ตัวอย่าง source code เป็นเพียง starter สำหรับ catalog GET endpoint ที่ **ยังไม่มี authentication** และต้องอยู่ในสภาพแวดล้อม development เท่านั้น การปล่อย production ต้องเพิ่ม auth, tests, secrets management, monitoring, CI และปรับ business rules ก่อน

## แหล่งอ้างอิง

- Tauri 2: https://v2.tauri.app/start/
- Tauri Vite config: https://v2.tauri.app/start/frontend/vite/
- Tauri capabilities: https://v2.tauri.app/security/capabilities/
- Tauri CSP: https://v2.tauri.app/security/csp/
- Native app OAuth RFC 8252: https://www.rfc-editor.org/rfc/rfc8252
- Axum: https://docs.rs/axum/latest/axum/
- Diesel: https://diesel.rs/
- OpenTelemetry Rust: https://opentelemetry.io/docs/languages/rust/
