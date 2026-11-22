# Nexa Commerce — Rust + Tauri + React Microservices Design Package

**Project name:** Nexa Commerce  
**Architecture:** Domain-Driven Design / Database-per-Service / Event-Driven / Saga Checkout  
**Scope:** Design documentation and a runnable commerce sandbox with Web Storefront, Tauri Desktop, and Rust Gateway.

## เอกสารออกแบบ

| Folder | Contents |
| --- | --- |
| `docs-desige/01_Architecture/` | Architecture PNG, SVG, editable Mermaid |
| `docs-desige/02_Database_ERD/` | Logical ERD PNG/SVG, dbdiagram.io DBML, Mermaid ERD for each of 8 services |
| `docs-desige/03_Checkout_and_Events/` | Checkout Saga Mermaid + Event Catalog CSV |
| `docs-desige/04_Documentation/` | Technical design document and database ownership CSV |
| `docs-desige/05_Django_Oscar_Reference/` | Original Django Oscar core-model ERD diagrams and research reference |

## วิธีใช้งาน

1. ดูภาพรวมได้ที่ `docs-desige/01_Architecture/Nexa_Commerce_Architecture.png`.
2. เปิด `docs-desige/02_Database_ERD/Nexa_Commerce_Schema.dbml` ที่ https://dbdiagram.io/ เพื่อดู/แก้ไข logical schema.
3. เปิดไฟล์ `.mmd` ด้วย Mermaid Live Editor (https://mermaid.live/) หรือ VS Code Mermaid extension.
4. อ่าน `docs-desige/04_Documentation/Nexa_Commerce_Technical_Design.md` สำหรับ API, events, Saga, failure handling และ migration phases.
5. ใช้ `docs-desige/05_Django_Oscar_Reference/` เป็นเอกสาร mapping จากโครงการ Django Oscar เดิมเท่านั้น ไม่ใช่ schema ของ Nexa Commerce.

## 8 Core Services

1. Customer (`customer_db`)
2. Catalog (`catalog_db`)
3. Pricing & Promotion (`pricing_db`)
4. Inventory (`inventory_db`)
5. Cart (`cart_db`)
6. Order & Checkout (`order_db`)
7. Payment (`payment_db`)
8. Fulfillment (`fulfillment_db`)

Supporting capabilities: API Gateway/BFF, OIDC IdP, Event Bus, Search projections, Notification worker.

## Important assumptions

- Separate database ownership; **no cross-database foreign keys**.
- External resource identifiers are UUID references, not SQL foreign keys.
- Orders retain immutable price / item / address snapshots.
- Checkout uses inventory reservation, payment authorization, outbox and idempotency.
- API schemas, event payloads, PCI handling, deployment, performance targets, tax/shipping rules, and concurrency constraints need validation before production.

## Attribution / source

The **Nexa Commerce** target design is a new proposal adapted from the **Django Oscar** e-commerce concepts. Original upstream repository: https://github.com/django-oscar/django-oscar. The legacy references are included for traceability; they should not be mistaken for the target implementation.

## ใหม่: Rust + Tauri + React Full-Stack Design

ไฟล์ใหม่อยู่ใน `docs-desige/06_Rust_Tauri_React_Fullstack/`:

- Full-stack Architecture PNG/SVG/DOT/Mermaid
- Shared Frontend / Desktop Transport Diagram (Web fetch vs Tauri Rust IPC)
- Rust Axum + Tokio + Diesel Technical Decisions, Monorepo Structure
- API/OpenAPI draft + OrderConfirmed event JSON Schema
- Tauri PKCE/OIDC, scoped capabilities, CSP, signed updater, offline strategy
- Infrastructure, CI/CD, implementation roadmap
- Runnable workspace in `project/` with Keycloak customer sign-in, cart, sandbox checkout, operator fulfillment, catalog search, and recovery and observability tools.

เริ่มอ่าน [คู่มือการออกแบบ Full-Stack](docs-desige/06_Rust_Tauri_React_Fullstack/02_Documentation/README_TH.md)


## เริ่มรันและทำงานกับ agents

- วิธีรัน Rust Gateway, React Storefront และ Tauri Desktop อยู่ใน [คู่มือรันโปรเจกต์](project/README.md)
- วิธีแบ่งงานและตรวจผลรวมสำหรับ lead agent และ subagents อยู่ใน [AGENTS.md](AGENTS.md)
