pub async fn sigterm() -> Result<Option<()>, std::io::Error> {
    #[cfg(unix)]
    let mut sigterm_handler =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    return Ok(sigterm_handler.recv().await);
    #[cfg(not(unix))]
    std::future::pending::<None>().await
}
