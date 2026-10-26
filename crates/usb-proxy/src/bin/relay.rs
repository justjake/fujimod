//! Run with FUJIMOD_USB_PROXY_TOKEN, behind an HTTPS reverse proxy.
//! FUJIMOD_USB_PROXY_DEV_SHARES=DIR enables the local-development share registry.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5202".into());
    let token = match std::env::var("FUJIMOD_USB_PROXY_TOKEN_FILE") {
        Ok(path) => std::fs::read_to_string(path)?.trim().to_owned(),
        Err(_) => std::env::var("FUJIMOD_USB_PROXY_TOKEN")?,
    };
    let origins: Vec<String> = std::env::var("FUJIMOD_USB_PROXY_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let mut app = fujimod_usb_proxy::relay::router(token, origins.clone())?;
    // Local development only: pages that share a camera register their link.
    if let Ok(directory) = std::env::var("FUJIMOD_USB_PROXY_DEV_SHARES") {
        app = app.merge(fujimod_usb_proxy::dev_shares::router(
            directory.clone().into(),
            origins,
        )?);
        println!("Development share registry: {directory}");
    }
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!("USB relay listening on {bind} at /usb-proxy/");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
