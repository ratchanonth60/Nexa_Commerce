use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::auth::DesktopSession;

const CARTS_URL: &str = "http://127.0.0.1:8082/api/v1/carts";

#[derive(Deserialize, Serialize)]
pub(crate) struct Cart {
    id: String,
    version: i64,
    items: Vec<CartItem>,
}

#[derive(Deserialize, Serialize)]
struct CartItem {
    product_id: String,
    quantity: i32,
}

#[tauri::command]
pub(crate) async fn create_cart(session: State<'_, DesktopSession>) -> Result<Cart, String> {
    send(&session, Method::POST, CARTS_URL, None).await
}

#[tauri::command]
pub(crate) async fn get_cart(
    session: State<'_, DesktopSession>,
    cart_id: String,
) -> Result<Cart, String> {
    let cart_id = Uuid::parse_str(&cart_id).map_err(|_| "Invalid cart ID.")?;
    send(
        &session,
        Method::GET,
        &format!("{CARTS_URL}/{cart_id}"),
        None,
    )
    .await
}

#[tauri::command]
pub(crate) async fn add_cart_item(
    session: State<'_, DesktopSession>,
    cart_id: String,
    product_id: String,
    quantity: i32,
) -> Result<Cart, String> {
    let cart_id = Uuid::parse_str(&cart_id).map_err(|_| "Invalid cart ID.")?;
    let product_id = Uuid::parse_str(&product_id).map_err(|_| "Invalid product ID.")?;
    if !(1..=9999).contains(&quantity) {
        return Err("Cart item quantity must be between 1 and 9999.".into());
    }
    let item = CartItem {
        product_id: product_id.to_string(),
        quantity,
    };
    send(
        &session,
        Method::POST,
        &format!("{CARTS_URL}/{cart_id}/items"),
        Some(&item),
    )
    .await
}

async fn send(
    session: &DesktopSession,
    method: Method,
    url: &str,
    item: Option<&CartItem>,
) -> Result<Cart, String> {
    let token = session.bearer()?;
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Could not initialize the cart client.")?
        .request(method, url)
        .bearer_auth(token);
    if let Some(item) = item {
        request = request.json(item);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Could not reach the cart service.")?;
    let error = match response.status() {
        status if status.is_success() => {
            return response
                .json()
                .await
                .map_err(|_| "The cart service returned an invalid response.".into());
        }
        StatusCode::BAD_REQUEST => "The cart request was invalid.",
        StatusCode::UNAUTHORIZED => {
            "The cart service did not accept this session. Sign out and sign in again."
        }
        StatusCode::FORBIDDEN => "This cart belongs to another customer.",
        StatusCode::NOT_FOUND => "The cart or product was not found.",
        StatusCode::CONFLICT => "The total quantity for this product would exceed 9999.",
        StatusCode::SERVICE_UNAVAILABLE => "The cart service is unavailable.",
        _ => "The cart request could not be completed.",
    };
    Err(error.into())
}
