# สถานะ Roadmap ของ Nexa Commerce

**สถานะปัจจุบัน** อยู่ที่ Phase 4 สำหรับเตรียม release และตรวจบน staging โดย Phase 1–3 ยังมีเกณฑ์รับงานบางส่วนที่ต้องปิด

| Phase | สถานะ | ทำแล้ว | เกณฑ์ที่ยังเปิด |
| --- | --- | --- | --- |
| 0 — Foundation | เสร็จ | Rust Axum, React, Tauri, PostgreSQL, OpenAPI และ GitHub Actions ใช้ Gateway สำหรับ Catalog ร่วมกัน | — |
| 1 — Commerce read & cart | ผ่านบนเว็บ | Catalog, ราคา, สต็อก, ตะกร้าตรวจเจ้าของ และ Keycloak OIDC ทดสอบผ่านบนเว็บ มี Rust IPC และ build สำหรับ Tauri | ทดสอบ login และตะกร้าผ่าน macOS WebView แบบโต้ตอบ |
| 2 — Checkout vertical slice | ผ่านด้วยตัวจำลองการชำระเงิน | Checkout Saga, จองและคืนสต็อก, idempotency, order snapshots และ transactional outbox ทดสอบการลองซ้ำและการชดเชยเมื่อชำระไม่ผ่านแล้ว | เชื่อมและทดสอบกับ PSP sandbox จริงตามเกณฑ์ roadmap |
| 3 — Operations | ผ่านบนเว็บ | ค้นหาสินค้า, สิทธิ์ operator, audit การแก้สต็อก, shipment และประวัติแจ้งเตือน ทดสอบเส้นทางจัดส่งจนลูกค้าเห็นสถานะผ่านเว็บแล้ว | ทดสอบ Tauri backoffice ผ่าน macOS WebView แบบโต้ตอบ |
| 4 — Production hardening | กำลังดำเนินการ | มี readiness, Prometheus, SLO rules, load smoke test, สคริปต์กู้คืนฐานข้อมูล และ workflow สร้าง Tauri updater แบบลงลายเซ็น | รัน recovery drill บน Docker จริง, ทดสอบติดตั้ง signed update, ตรวจ security และ release checklist, เปิดติดตาม SLO และทดสอบแจ้งเตือนบน staging |

## งานเปิดตามลำดับที่ทำต่อได้

- [ ] ทดสอบ Tauri login, ตะกร้า และ backoffice กับ Keycloak บน macOS WebView
- [ ] รัน recovery drill บน Docker Compose และตรวจการแยกสภาพแวดล้อมกับ cleanup
- [ ] เลือก PSP และทดสอบ payment sandbox กับ webhook ของผู้ให้บริการ
- [ ] ตั้งค่า GitHub environment `desktop-release` และทดสอบอัปเดตจากรุ่นก่อนหน้าด้วย artifact ที่เซ็นแล้ว
- [ ] เปิด staging SLO monitoring และทดสอบเส้นทางแจ้งเตือน
- [ ] ปิด security review และ release checklist รวมถึงยืนยันการตั้งค่า production IdP และการหมุนเวียนคีย์

ผลกู้คืนที่มีใช้ PostgreSQL จริงกับ Docker CLI shim ส่วนการตรวจ Tauri ล่าสุดครอบคลุม build และ Clippy สำหรับ updater feature

[Roadmap ต้นฉบับ](docs-desige/06_Rust_Tauri_React_Fullstack/02_Documentation/06_Implementation_Roadmap.md) · [คู่มือรันโปรเจกต์](project/README.md) · [CI ผ่านสำหรับ d439d70](https://github.com/ratchanonth60/Nexa_Commerce/actions/runs/45015088120)
