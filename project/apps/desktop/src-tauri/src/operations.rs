use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::auth::DesktopSession;

const OPERATIONS_URL: &str = "http://127.0.0.1:8082/api/v1/ops";

#[derive(Deserialize, Serialize)]
pub(crate) struct Stock {
    product_id: String,
    on_hand: i32,
    reserved: i32,
    allocated: i32,
}

#[derive(Serialize)]
struct StockAdjustment {
    expected_on_hand: i32,
    on_hand: i32,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShipmentStatus {
    Pending,
    Packed,
    Dispatched,
    Delivered,
}

impl ShipmentStatus {
    fn can_advance_to(self, status: Self) -> bool {
        matches!(
            (self, status),
            (Self::Pending, Self::Packed)
                | (Self::Packed, Self::Dispatched)
                | (Self::Dispatched, Self::Delivered)
        )
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Shipment {
    id: String,
    order_id: String,
    status: ShipmentStatus,
}

#[derive(Serialize)]
struct ShipmentChange {
    expected_status: ShipmentStatus,
    status: ShipmentStatus,
}

#[derive(Serialize)]
pub(crate) struct ShipmentAdvance {
    shipment: Shipment,
    queued: bool,
}

#[tauri::command]
pub(crate) async fn read_stock(
    session: State<'_, DesktopSession>,
    product_id: String,
) -> Result<Stock, String> {
    let product_id = Uuid::parse_str(&product_id).map_err(|_| "Enter a valid product UUID.")?;
    send(request(&session, Method::GET, "inventory", product_id)?)
        .await
        .map(|(_, stock)| stock)
}

#[tauri::command]
pub(crate) async fn adjust_stock(
    session: State<'_, DesktopSession>,
    product_id: String,
    expected_on_hand: i32,
    on_hand: i32,
) -> Result<Stock, String> {
    let product_id = Uuid::parse_str(&product_id).map_err(|_| "Enter a valid product UUID.")?;
    if expected_on_hand < 0 || on_hand < 0 {
        return Err("Stock quantities must be nonnegative.".into());
    }
    let change = StockAdjustment {
        expected_on_hand,
        on_hand,
    };
    send(request(&session, Method::PATCH, "inventory", product_id)?.json(&change))
        .await
        .map(|(_, stock)| stock)
}

#[tauri::command]
pub(crate) async fn read_shipment(
    session: State<'_, DesktopSession>,
    order_id: String,
) -> Result<Shipment, String> {
    let order_id = Uuid::parse_str(&order_id).map_err(|_| "Enter a valid order UUID.")?;
    send(request(&session, Method::GET, "shipments", order_id)?)
        .await
        .map(|(_, shipment)| shipment)
}

#[tauri::command]
pub(crate) async fn advance_shipment(
    session: State<'_, DesktopSession>,
    order_id: String,
    expected_status: ShipmentStatus,
    status: ShipmentStatus,
) -> Result<ShipmentAdvance, String> {
    let order_id = Uuid::parse_str(&order_id).map_err(|_| "Enter a valid order UUID.")?;
    if !expected_status.can_advance_to(status) {
        return Err("That shipment status change is unavailable.".into());
    }
    let change = ShipmentChange {
        expected_status,
        status,
    };
    let (status, shipment) =
        send(request(&session, Method::POST, "shipments", order_id)?.json(&change)).await?;
    Ok(ShipmentAdvance {
        shipment,
        queued: status == StatusCode::ACCEPTED,
    })
}

fn request(
    session: &DesktopSession,
    method: Method,
    resource: &str,
    id: Uuid,
) -> Result<RequestBuilder, String> {
    let token = session.operator_bearer()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Could not initialize the backoffice client.")?;
    Ok(client
        .request(method, format!("{OPERATIONS_URL}/{resource}/{id}"))
        .bearer_auth(token))
}

async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<(StatusCode, T), String> {
    let response = request
        .send()
        .await
        .map_err(|_| "Could not reach the backoffice service.")?;
    let error = match response.status() {
        status if status.is_success() => {
            return response
                .json()
                .await
                .map(|body| (status, body))
                .map_err(|_| "The backoffice service returned an invalid response.".into());
        }
        StatusCode::BAD_REQUEST => "The backoffice request was invalid.",
        StatusCode::UNAUTHORIZED => {
            "The backoffice session was rejected. Sign out and sign in again."
        }
        StatusCode::FORBIDDEN => "Operator access is required.",
        StatusCode::NOT_FOUND => "No matching record was found.",
        StatusCode::CONFLICT => {
            "The change conflicts with current records. Load the latest values and try again."
        }
        StatusCode::SERVICE_UNAVAILABLE => "The backoffice service is unavailable.",
        _ => "The backoffice request could not be completed.",
    };
    Err(error.into())
}
